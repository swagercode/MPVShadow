use std::fs::OpenOptions;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use std::thread::sleep;
use serde_json::Value;
use anyhow::{Result, Context};
use byteorder::{ByteOrder, LittleEndian};
use url::Url;
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoop, EventLoopProxy},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

#[derive(Debug, Clone)]
struct UiPayload {
    text: Option<String>,
    s: f64,
    e: f64,
    dur: f64,
    ff_index: Option<u64>,
    out_path: String,
    latency_ms: u64,
    rms: f32,
    peak: f32,
}
use std::sync::{Arc, Mutex};


// Send one JSON command (newline-delimited) to mpv IPC
fn send_cmd(writer: &mut std::fs::File, v: serde_json::Value) -> io::Result<()> {
    let s = v.to_string();
    writer.write_all(s.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

// Read lines until a reply carrying the matching request_id is seen
fn read_reply_with_id(reader: &mut BufReader<std::fs::File>, request_id: u64) -> io::Result<Value> {
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "mpv closed pipe"));
        }
        if let Ok(v) = serde_json::from_str::<Value>(&line) {
            if v.get("request_id").and_then(|x| x.as_u64()) == Some(request_id) {
                return Ok(v);
            }
            // otherwise: unrelated event/reply, keep looping
        }
    }
}

// Get the audio stream and open input
fn build_ffmpeg_base_args(media_path: &str, start_s: f64, end_s: f64, ff_index: Option<u64>) -> Vec<String> {
    let mut args = Vec::new();
    args.push("-hide_banner".to_string());
    args.push("-nostdin".to_string());
    args.push("-ss".to_string());
    args.push(format!("{:.3}", start_s));
    args.push("-to".to_string());
    args.push(format!("{:.3}", end_s));
    args.push("-i".to_string());
    args.push(media_path.to_string());
    if let Some(idx) = ff_index {
        args.push("-map".to_string());
        args.push(format!("0:{}", idx));
    }
    args
}

fn spawn_wav_writer(base_args: &[String], out_path: &Path) {
    let mut args = base_args.to_vec();
    args.push("-vn".to_string());
    args.push("-sn".to_string());
    args.push("-c:a".to_string());
    args.push("pcm_s16le".to_string());
    args.push("-ar".to_string());
    args.push("48000".to_string());
    args.push("-ac".to_string());
    args.push("2".to_string());
    args.push(out_path.to_string_lossy().to_string());

    match Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn() {
        Ok(_child) => { /* intentionally not awaited */ }
        Err(e) => {
            eprintln!("ffmpeg wav spawn error: {}", e);
        }
    }
}

fn spawn_pcm_pipe(base_args: &[String]) -> Result<(Child, ChildStdout)> {
    let mut args = base_args.to_vec();
    args.push("-vn".to_string());
    args.push("-sn".to_string());
    args.push("-f".to_string());
    args.push("f32le".to_string());
    args.push("-ar".to_string());
    args.push("48000".to_string());
    args.push("-ac".to_string());
    args.push("2".to_string());
    args.push("pipe:1".to_string());

    let mut cmd = Command::new("ffmpeg");
    let mut child = cmd
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "failed to spawn ffmpeg for pcm pipe")?;

    let stdout = child.stdout.take().context("failed to take stdout")?;
    Ok((child, stdout))
}

// Convenience: issue get_property and wait for its reply
fn get_property(reader: &mut BufReader<std::fs::File>, writer: &mut std::fs::File, request_id: u64, name: &str) -> io::Result<Value> {
    let cmd = serde_json::json!({
        "request_id": request_id,
        "command": ["get_property", name]
    });
    send_cmd(writer, cmd)?;
    read_reply_with_id(reader, request_id)
}

fn run_analyzer(proxy: EventLoopProxy<()>, shared: Arc<Mutex<Option<UiPayload>>>) {
    let pipe_path = r"\\.\\pipe\\MPVShadow";

    // Connect to the mpv JSON IPC named pipe (retry until mpv is up)
    let file = loop {
        match OpenOptions::new().read(true).write(true).open(pipe_path) {
            Ok(f) => break f,
            Err(_) => {
                sleep(Duration::from_millis(300));
            }
        }
    };

    // Split into reader/writer handles
    let mut reader = BufReader::new(file.try_clone().expect("clone pipe handle"));
    let mut writer = file;

    // Subscribe to client-message events so we see script-message triggers
    let subscribe = serde_json::json!({
        "command": ["request_event", "client-message", true]
    });
    let _ = send_cmd(&mut writer, subscribe);

    // Read incoming lines and look for our trigger
    let mut line_buf = String::new();
    loop {
        line_buf.clear();
        let Ok(n) = reader.read_line(&mut line_buf) else { break };
        if n == 0 { break; }
        let Ok(v): Result<Value, _> = serde_json::from_str(&line_buf) else { continue };

        if v.get("event") == Some(&Value::String("client-message".into())) {
            eprintln!("client-message: {:?}", v);
            if let Some(args) = v.get("args").and_then(|a| a.as_array()) {
                if args.first().and_then(|x| x.as_str()) == Some("cut_current_sub") {
                    eprintln!("trigger: cut_current_sub");
                    // Query properties (sequential; replies may interleave with events but we filter by request_id)
                    let sub_text = get_property(&mut reader, &mut writer, 1, "sub-text").ok();
                    let sub_start = get_property(&mut reader, &mut writer, 2, "sub-start").ok();
                    let sub_end = get_property(&mut reader, &mut writer, 3, "sub-end").ok();
                    let duration = get_property(&mut reader, &mut writer, 4, "duration").ok();
                    let _path = get_property(&mut reader, &mut writer, 5, "path").ok();
                    let track_list = get_property(&mut reader, &mut writer, 6, "track-list").ok();

                    let text = sub_text.and_then(|v| v.get("data").cloned()).and_then(|d| d.as_str().map(|s| s.to_string()));
                    let mut s = sub_start.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);
                    let mut e = sub_end.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);
                    let dur = duration.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);
                    eprintln!("text: {:?}", text);
                    eprintln!("s: {:?}", s);
                    eprintln!("e: {:?}", e);
                    eprintln!("dur: {:?}", dur);

                    // Padding + clamping
                    let pad = 0.10f64;
                    if s > pad { s -= pad; } else { s = 0.0; }
                    e += pad;
                    if dur > 0.0 && e > dur { e = dur; }

                    // read selected audio ff-index
                    let mut ff_index: Option<u64> = None;
                    if let Some(tl) = track_list.and_then(|v| v.get("data").cloned()) {
                        if let Some(arr) = tl.as_array() {
                            for t in arr {
                                let is_audio = t.get("type").and_then(|x| x.as_str()) == Some("audio");
                                let selected = t.get("selected").and_then(|x| x.as_bool()) == Some(true);
                                if is_audio && selected {
                                    ff_index = t.get("ff-index").and_then(|x| x.as_u64());
                                    eprintln!("ff-index: {:?}", ff_index);
                                    break;
                                }
                            }
                        }
                    }


                    // create output directory
                    let out_dir = std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()).join("shadow_out");
                    let _ = std::fs::create_dir_all(&out_dir);
                    let media_path = _path
                        .as_ref()
                        .and_then(|v| v.get("data")
                        .and_then(|d| d.as_str()))
                        .map(|s| s.to_owned())
                        .unwrap_or_else(|| "<unknown>".to_string());
                    let base = std::path::Path::new(&media_path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("clip");
                    let start_ms = (s * 1000.0).round() as u64;
                    let end_ms = (e * 1000.0).round() as u64;
                    let out_path = out_dir.join(format!("{}_{}_{}.wav", base, start_ms, end_ms));
                    eprintln!("out_path: {:?}", out_path);
                    
                    // Spawn external ffmpeg to write WAV in the background (non-blocking)
                    if media_path != "<unknown>" && s < e {
                        let base_args = build_ffmpeg_base_args(&media_path, s, e, ff_index);
                        spawn_wav_writer(&base_args, &out_path);

                        // Spawn external ffmpeg to pipe f32le PCM to stdout and analyze a small chunk
                        let start_instant = Instant::now();
                        match spawn_pcm_pipe(&base_args) {
                            Ok((_child, mut stdout)) => {
                                let (tx, rx) = mpsc::channel();
                                thread::spawn(move || {
                                    let frames: usize = 4096; // per channel
                                    let bytes_needed: usize = frames * 2 * 4; // 2ch * 4 bytes per f32
                                    let mut buf = vec![0u8; bytes_needed];
                                    // Blocking read; first non-zero read marks first-byte latency
                                    match stdout.read(&mut buf) {
                                        Ok(n) if n > 0 => {
                                            let first_latency_ms = start_instant.elapsed().as_millis() as u64;
                                            let sample_count = n / 4; // bytes to f32 samples (both channels interleaved)
                                            let mut samples = vec![0f32; sample_count];
                                            LittleEndian::read_f32_into(&buf[..sample_count * 4], &mut samples);
                                            let mut sum_sq: f64 = 0.0;
                                            let mut peak_abs: f32 = 0.0;
                                            for &x in &samples {
                                                let ax = x.abs();
                                                if ax > peak_abs { peak_abs = ax; }
                                                sum_sq += (x as f64) * (x as f64);
                                            }
                                            let rms = if sample_count > 0 {
                                                (sum_sq / sample_count as f64).sqrt() as f32
                                            } else { 0.0 };
                                            let _ = tx.send(Ok((first_latency_ms, rms, peak_abs)));
                                        }
                                        Ok(_) => {
                                            let _ = tx.send(Err("ffmpeg pipe returned 0 bytes".to_string()));
                                        }
                                        Err(e) => {
                                            let _ = tx.send(Err(format!("ffmpeg pipe read error: {}", e)));
                                        }
                                    }
                                });

                                match rx.recv_timeout(Duration::from_millis(200)) {
                                    Ok(Ok((lat, rms, peak))) => {
                                        eprintln!("first-byte latency: {} ms; rms={:.4} peak={:.4}", lat, rms, peak);
                                        // Notify UI
                                        let payload = UiPayload {
                                            text: text.clone(),
                                            s,
                                            e,
                                            dur,
                                            ff_index,
                                            out_path: out_path.to_string_lossy().to_string(),
                                            latency_ms: lat,
                                            rms,
                                            peak,
                                        };
                                        if let Ok(mut guard) = shared.lock() { *guard = Some(payload); }
                                        let _ = proxy.send_event(());
                                    }
                                    Ok(Err(msg)) => {
                                        eprintln!("pcm analysis error: {}", msg);
                                    }
                                    Err(_) => {
                                        eprintln!("pcm analysis timeout waiting for first bytes");
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("ffmpeg pcm spawn error: {}", e);
                            }
                        }
                    } else if media_path == "<unknown>" {
                        eprintln!("no active subtitle or unknown media path");
                    }

                    // Show quick OSD confirmation
                    let msg = if text.is_some() && s < e {
                        format!("cut {:.3}–{:.3} (ff={:?})", s, e, ff_index)
                    } else {
                        "no active subtitle".to_string()
                    };
                    let _ = send_cmd(&mut writer, serde_json::json!({
                        "command": ["show-text", msg, 1200]
                    }));
                    
                }
            }
        }
    }
}

fn main() {
    let event_loop: EventLoop<()> = EventLoop::new();
    let proxy = event_loop.create_proxy();
    let shared: Arc<Mutex<Option<UiPayload>>> = Arc::new(Mutex::new(None));

    let window = WindowBuilder::new()
        .with_title("MPV Shadow")
        .with_inner_size(LogicalSize::new(420.0, 320.0))
        .build(&event_loop)
        .expect("failed to build window");

    // Build a file:// URL to assets/index.html located at crate_dir/assets/index.html
    let exe = std::env::current_exe().unwrap_or_else(|_| std::env::current_dir().unwrap());
    let assets_dir = exe
        .parent() // .../target/release
        .and_then(|p| p.parent()) // .../target
        .and_then(|p| p.parent()) // .../shadow_analyzer
        .map(|p| p.join("assets"))
        .unwrap_or_else(|| std::path::PathBuf::from("assets"));
    let index_path = assets_dir.join("index.html");
    let file_url = Url::from_file_path(&index_path).expect("valid file url for index.html");

    let webview = WebViewBuilder::new(&window)
        .with_url(file_url.as_str())
        .with_devtools(true)
        .build()
        .expect("build webview");

    {
        let shared_an = Arc::clone(&shared);
        thread::spawn(move || run_analyzer(proxy, shared_an));
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {}
            Event::UserEvent(()) => {
                if let Ok(mut guard) = shared.lock() {
                    if let Some(p) = guard.take() {
                        if let Ok(js) = serde_json::to_string(&serde_json::json!({
                            "text": p.text,
                            "s": p.s,
                            "e": p.e,
                            "dur": p.dur,
                            "ff_index": p.ff_index,
                            "out_path": p.out_path,
                            "latency_ms": p.latency_ms,
                            "rms": p.rms,
                            "peak": p.peak,
                        })) {
                            let _ = webview.evaluate_script(&format!(
                                "window.dispatchEvent(new CustomEvent('analysis', {{ detail: {} }}));",
                                js
                            ));
                        }
                    }
                }
            }
            Event::WindowEvent { event, .. } => {
                match event {
                    WindowEvent::CloseRequested => {
                        *control_flow = ControlFlow::Exit;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}

