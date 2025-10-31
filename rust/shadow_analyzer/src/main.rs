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
use windows::Win32::Media::Audio::{DEVICE_STATE_ACTIVE, EDataFlow, IMMDeviceCollection, IMMDeviceEnumerator};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

#[derive(Debug, Clone)]
struct UiPayload {
    text: Option<String>,
    s: f64,
    e: f64,
    dur: f64,
    ff_index: Option<u64>,
    out_path: String,
    latest_path: String,
    // Optional microphone outputs
    latest_mic_path: Option<String>,
    mic_out_path: Option<String>,
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
    args.push("-loglevel".to_string());
    args.push("error".to_string());
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

fn spawn_wav_writer(base_args: &[String], out_path: &Path, overwrite: bool) {
    let mut args = base_args.to_vec();
    if overwrite {
        args.insert(0, "-y".to_string());
    }
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
        Ok(mut child) => {
            // Supervise exit in the background; do not block fast path
            thread::spawn(move || {
                match child.wait() {
                    Ok(status) => {
                        if !status.success() {
                            eprintln!("ffmpeg wav exited with status {:?}", status.code());
                        }
                    }
                    Err(e) => eprintln!("ffmpeg wav wait error: {}", e),
                }
            });
        }
        Err(e) => {
            eprintln!("ffmpeg wav spawn error: {}", e);
        }
    }
}
fn cleanup_old_clips(out_dir: &Path, keep: usize, exclude: &[&Path]) {
    let dir = out_dir.to_path_buf();
    let exclude: Vec<std::path::PathBuf> = exclude.iter().map(|p| p.to_path_buf()).collect();
    thread::spawn(move || {
        let Ok(read_dir) = std::fs::read_dir(&dir) else { return };
        let mut entries: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
        for e in read_dir.flatten() {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("wav") { continue; }
            if exclude.iter().any(|ex| ex == &path) { continue; }
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.eq_ignore_ascii_case("latest.wav") { continue; }
            let Ok(meta) = e.metadata() else { continue };
            let Ok(modified) = meta.modified() else { continue };
            entries.push((path, modified));
        }
        // Sort newest first
        entries.sort_by(|a, b| b.1.cmp(&a.1));
        if entries.len() > keep {
            for (path, _) in entries.into_iter().skip(keep) {
                let _ = std::fs::remove_file(&path);
            }
        }
    });
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

#[derive(Clone, Debug, serde::Serialize)]
struct MicDeviceInfo { id: String, name: String }

// Prefer DirectShow device names (what ffmpeg expects), fallback to WASAPI GUIDs
fn list_mic_devices_dshow() -> Option<Vec<MicDeviceInfo>> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-f", "dshow", "-list_devices", "true", "-i", "dummy"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    let mut out: Vec<MicDeviceInfo> = Vec::new();
    for line in stderr_text.lines() {
        // Skip alternative moniker lines; we want human-friendly names
        if line.contains("Alternative name") { continue; }
        // We only care about audio device entries
        if !line.contains("(audio)") { continue; }
        // Extract quoted device name
        if let Some(start) = line.find('"') {
            if let Some(end_rel) = line[start+1..].find('"') {
                let name = &line[start+1..start+1+end_rel];
                if !name.is_empty() {
                    let id = format!("audio={}", name);
                    out.push(MicDeviceInfo { id, name: name.to_string() });
                }
            }
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn list_mic_devices() -> Vec<MicDeviceInfo> {
    if let Some(list) = list_mic_devices_dshow() { return list; }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&windows::Win32::Media::Audio::MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let collection: IMMDeviceCollection = enumerator.EnumAudioEndpoints(EDataFlow(1), DEVICE_STATE_ACTIVE).unwrap(); // eCapture
        let count = collection.GetCount().unwrap_or(0);
        let mut out = Vec::new();
        for i in 0..count {
            if let Ok(dev) = collection.Item(i) {
                if let Ok(pw) = dev.GetId() {
                    let id = pw.to_string().unwrap_or_default();
                    if !id.is_empty() {
                        // Friendly name fallback: use ID if we couldn't parse dshow list
                        out.push(MicDeviceInfo { id: id.clone(), name: id });
                    }
                }
            }
        }
        out
    }
}

fn spawn_mic_recorder(
    latest_path: &Path,
    unique_path: &Path,
    duration_s: f64,
    device: &str,
    out_dir: &Path,
    proxy: EventLoopProxy<()>,
    shared: Arc<Mutex<Option<UiPayload>>>,
    // snapshot of fields to resend on completion
    text: Option<String>,
    s: f64,
    e: f64,
    dur: f64,
    ff_index: Option<u64>,
    out_path: String,
    latest_src_path: String,
    latency_ms: u64,
    rms: f32,
    peak: f32,
) {
    let mut args: Vec<String> = Vec::new();
    args.push("-hide_banner".to_string());
    args.push("-loglevel".to_string());
    args.push("error".to_string());
    args.push("-nostdin".to_string());
    args.push("-f".to_string());
    args.push("dshow".to_string());
    args.push("-i".to_string());
    args.push(device.to_string());
    args.push("-ss".to_string());
    args.push("0".to_string());
    args.push("-t".to_string());
    args.push(format!("{:.3}", duration_s.max(0.0)));
    args.push("-ar".to_string());
    args.push("48000".to_string());
    args.push("-ac".to_string());
    args.push("1".to_string());
    args.push("-c:a".to_string());
    args.push("pcm_s16le".to_string());
    args.push("-y".to_string());
    args.push(latest_path.to_string_lossy().to_string());

    match Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            let latest_path = latest_path.to_path_buf();
            let unique_path = unique_path.to_path_buf();
            let out_dir = out_dir.to_path_buf();
            thread::spawn(move || {
                // Wait for process, then copy and cleanup
                match child.wait() {
                    Ok(status) => {
                        if !status.success() {
                            eprintln!("ffmpeg mic exited with status {:?}", status.code());
                        }
                    }
                    Err(e) => eprintln!("ffmpeg mic wait error: {}", e),
                }
                // Copy latest to unique (best-effort)
                if let Err(e) = std::fs::copy(&latest_path, &unique_path) {
                    eprintln!("copy latest_mic -> unique error: {}", e);
                }
                // Cleanup retention for mic wavs
                cleanup_old_clips(&out_dir, 5, &[&latest_path, &unique_path]);

                // Dispatch follow-up UI event with mic paths
                let payload = UiPayload {
                    text: text.clone(),
                    s,
                    e,
                    dur,
                    ff_index,
                    out_path: out_path.clone(),
                    latest_path: latest_src_path.clone(),
                    latest_mic_path: Some(latest_path.to_string_lossy().to_string()),
                    mic_out_path: Some(unique_path.to_string_lossy().to_string()),
                    latency_ms,
                    rms,
                    peak,
                };
                if let Ok(mut guard) = shared.lock() { *guard = Some(payload); }
                let _ = proxy.send_event(());
            });
        }
        Err(e) => {
            eprintln!("ffmpeg mic spawn error: {}", e);
        }
    }
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

fn run_analyzer(proxy: EventLoopProxy<()>, shared: Arc<Mutex<Option<UiPayload>>>, mic_selected: Arc<Mutex<Option<String>>>) {
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
    // Observe subtitle changes to keep current_line updated continuously
    let _ = send_cmd(&mut writer, serde_json::json!({
        "command": ["observe_property", 201, "sub-text"]
    }));

    // Read incoming lines and look for our trigger
    let mut line_buf = String::new();
    // Playback watcher state: pause at this time if Some
    let mut watch_until: Option<f64> = None;
    let mut observing_timepos: bool = false;
    // Previous subtitle line (raw, without padding): (text, start, end)
    let mut current_line: Option<(Option<String>, f64, f64)> = None;
    //
    loop {
        line_buf.clear();
        let Ok(n) = reader.read_line(&mut line_buf) else { break };
        if n == 0 { break; }
        let Ok(v): Result<Value, _> = serde_json::from_str(&line_buf) else { continue };

        // Handle property-change for time-pos to enforce pause at end
        if v.get("event") == Some(&Value::String("property-change".into())) {
            if let Some(name) = v.get("name").and_then(|x| x.as_str()) {
                if name == "time-pos" {
                    if let (Some(t), Some(cur)) = (watch_until, v.get("data").and_then(|d| d.as_f64())) {
                        if cur >= t {
                            let _ = send_cmd(&mut writer, serde_json::json!({
                                "command": ["set_property", "pause", true]
                            }));
                            if observing_timepos {
                                let _ = send_cmd(&mut writer, serde_json::json!({
                                    "command": ["unobserve_property", 101]
                                }));
                                observing_timepos = false;
                            }
                            watch_until = None;
                        }
                    }
                } else if name == "sub-text" {
                    // Update current_line when a subtitle becomes visible
                    if let Some(text_val) = v.get("data").and_then(|d| d.as_str()).map(|s| s.to_string()) {
                        // Query sub-start and sub-end to capture window
                        let sub_start = get_property(&mut reader, &mut writer, 2001, "sub-start").ok();
                        let sub_end = get_property(&mut reader, &mut writer, 2002, "sub-end").ok();
                        let s_now = sub_start.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);
                        let e_now = sub_end.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);
                        if e_now > s_now {
                            current_line = Some((Some(text_val.clone()), s_now, e_now));
                            eprintln!("current_line updated: s={:.3} e={:.3}", s_now, e_now);
                        }
                    }
                }
            }
        }

        if v.get("event") == Some(&Value::String("client-message".into())) {
            eprintln!("client-message: {:?}", v);
            if let Some(args) = v.get("args").and_then(|a| a.as_array()) {
                if args.first().and_then(|x| x.as_str()) == Some("cut_current_sub") {
                    eprintln!("trigger: cut_current_sub");
                    // Query properties (sequential; replies may interleave with events but we filter by request_id)
                    let duration = get_property(&mut reader, &mut writer, 4, "duration").ok();
                    let _path = get_property(&mut reader, &mut writer, 5, "path").ok();
                    let track_list = get_property(&mut reader, &mut writer, 6, "track-list").ok();

                    let dur = duration.and_then(|v| v.get("data").and_then(|d| d.as_f64())).unwrap_or(0.0);

                    // Always use current_line (start/end from the last visible subtitle)
                    let (mut text, mut s, mut e) = match current_line.clone() {
                        Some((t, s0, e0)) => (t, s0, e0),
                        None => {
                            eprintln!("No current_line available; skipping cut");
                            continue;
                        }
                    };

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
                    let latest_path = out_dir.join("latest.wav");
                    eprintln!("out_path: {:?}", out_path);
                    
                    // Spawn external ffmpeg to write WAV in the background (non-blocking)
                    if media_path != "<unknown>" && s < e {
                        // Playback control: seek to s and unpause; pause again at e via watcher
                        let _ = send_cmd(&mut writer, serde_json::json!({
                            "command": ["set_property", "pause", true]
                        }));
                        let _ = send_cmd(&mut writer, serde_json::json!({
                            "command": ["set_property", "time-pos", s]
                        }));

                        // Set watcher threshold a bit before e
                        watch_until = Some((e - 0.02).max(0.0));
                        if !observing_timepos {
                            let _ = send_cmd(&mut writer, serde_json::json!({
                                "command": ["observe_property", 101, "time-pos"]
                            }));
                            observing_timepos = true;
                        }

                        // Prepare mic paths
                        let latest_mic_path = out_dir.join("latest_mic.wav");
                        let mic_out_path = out_dir.join(format!("{}_{}_{}_mic.wav", base, start_ms, end_ms));

                        let base_args = build_ffmpeg_base_args(&media_path, s, e, ff_index);
                        // unique clip
                        spawn_wav_writer(&base_args, &out_path, false);
                        // latest clip (overwrite)
                        spawn_wav_writer(&base_args, &latest_path, true);
                        // schedule retention cleanup (keep 5 unique clips)
                        cleanup_old_clips(&out_dir, 5, &[&out_path, &latest_path]);

                        // Start mic recorder: use selected device, else fallback to first detected
                        let mic_device_sel = mic_selected.lock().ok().and_then(|g| g.clone());
                        let mut chosen_dev: Option<String> = mic_device_sel.clone();
                        if chosen_dev.is_none() {
                            if let Some(list) = list_mic_devices_dshow() {
                                if let Some(first) = list.first() {
                                    eprintln!("No mic selected; falling back to first device: '{}'", first.name);
                                    chosen_dev = Some(first.id.clone());
                                }
                            }
                        }
                        if let Some(dev) = chosen_dev.as_deref() {
                            spawn_mic_recorder(
                                &latest_mic_path,
                                &mic_out_path,
                                (e - s).max(0.0),
                                dev,
                                &out_dir,
                                proxy.clone(),
                                Arc::clone(&shared),
                                text.clone(),
                                s,
                                e,
                                dur,
                                ff_index,
                                out_path.to_string_lossy().to_string(),
                                latest_path.to_string_lossy().to_string(),
                                0,
                                0.0,
                                0.0,
                            );

                            // Optional readiness: wait up to ~150ms for file to exist and have size > 44 bytes
                            let start_ready = Instant::now();
                            loop {
                                let meta = std::fs::metadata(&latest_mic_path);
                                if let Ok(m) = meta {
                                    if m.len() > 44 { break; }
                                }
                                if start_ready.elapsed() > Duration::from_millis(150) { break; }
                                sleep(Duration::from_millis(25));
                            }
                        } else {
                            eprintln!("No microphone available; skipping mic capture.");
                        }

                        // Unpause playback now
                        let _ = send_cmd(&mut writer, serde_json::json!({
                            "command": ["set_property", "pause", false]
                        }));

                        // Spawn external ffmpeg to pipe f32le PCM to stdout and analyze a small chunk
                        let start_instant = Instant::now();
                        match spawn_pcm_pipe(&base_args) {
                            Ok((mut child, mut stdout)) => {
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
                                            latest_path: latest_path.to_string_lossy().to_string(),
                                            latest_mic_path: mic_device_sel.as_ref().map(|_| latest_mic_path.to_string_lossy().to_string()),
                                            mic_out_path: mic_device_sel.as_ref().map(|_| mic_out_path.to_string_lossy().to_string()),
                                            latency_ms: lat,
                                            rms,
                                            peak,
                                        };
                                        if let Ok(mut guard) = shared.lock() { *guard = Some(payload); }
                                        let _ = proxy.send_event(());
                                    }
                                    Ok(Err(msg)) => {
                                        eprintln!("pcm analysis error: {}", msg);
                                        let _ = child.kill();
                                        let _ = child.wait();
                                    }
                                    Err(_) => {
                                        eprintln!("pcm analysis timeout waiting for first bytes");
                                        let _ = child.kill();
                                        let _ = child.wait();
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

        // Check for mic UI update injected by background (not used in this approach)
    }
}

fn main() {
    let event_loop: EventLoop<()> = EventLoop::new();
    let proxy = event_loop.create_proxy();
    let shared: Arc<Mutex<Option<UiPayload>>> = Arc::new(Mutex::new(None));
    let devices_shared: Arc<Mutex<Option<Vec<MicDeviceInfo>>>> = Arc::new(Mutex::new(None));
    let mic_selected: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

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

    let mic_selected_for_ipc = Arc::clone(&mic_selected);
    let webview = WebViewBuilder::new(&window)
        .with_url(file_url.as_str())
        .with_devtools(true)
        .with_ipc_handler(move |msg| {
            let body = msg.body();
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
                if v.get("type") == Some(&Value::String("mic_device".into())) {
                    if let Some(val) = v.get("value").and_then(|x| x.as_str()) {
                        if let Ok(mut g) = mic_selected_for_ipc.lock() {
                            *g = if val == "default" { None } else { Some(val.to_string()) };
                        }
                    }
                }
            }
        })
        .build()
        .expect("build webview");

    {
        let shared_an = Arc::clone(&shared);
        let mic_sel = Arc::clone(&mic_selected);
        let proxy_an = proxy.clone();
        thread::spawn(move || run_analyzer(proxy_an, shared_an, mic_sel));
    }

    {
        let devices_out = Arc::clone(&devices_shared);
        let proxy_dev = proxy.clone();
        thread::spawn(move || {
            eprintln!("Scanning for microphone devices...");
            let list = list_mic_devices();
            eprintln!("Detected {} microphone device(s) total", list.len());
            for d in &list {
                eprintln!("  id='{}' name='{}'", d.id, d.name);
            }
            if let Ok(mut g) = devices_out.lock() { *g = Some(list); }
            let _ = proxy_dev.send_event(());
        });
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
                            "latest_path": p.latest_path,
                            "latest_mic_path": p.latest_mic_path,
                            "mic_out_path": p.mic_out_path,
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
                if let Ok(mut dg) = devices_shared.lock() {
                    if let Some(list) = dg.take() {
                        if let Ok(js) = serde_json::to_string(&serde_json::json!({
                            "micDevices": list
                        })) {
                            let _ = webview.evaluate_script(&format!(
                                "window.dispatchEvent(new CustomEvent('devices', {{ detail: {} }}));",
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

