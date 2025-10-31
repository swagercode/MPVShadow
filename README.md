## MPVShadow (work in progress)

Minimal, fast subtitle-audio cutter + live UI for mpv on Windows.

- Press C while a subtitle is visible.
- The analyzer grabs the selected audio track from the playing file via mpv IPC, cuts a small window around the subtitle, writes a WAV to `shadow_out/`, and pipes a short PCM burst to compute quick metrics (RMS/peak). It also shows an OSD confirmation in mpv.
- A persistent webview window displays the latest line, window, track info, latency, RMS/peak, and a Play/Pause button for the WAV.

Status: actively evolving; interfaces and behavior may change.

### Current features
- mpv JSON IPC integration (named pipe `\\.\\pipe\\MPVShadow`)
- C key Lua trigger (`script-message cut_current_sub`)
- External ffmpeg for both:
  - WAV writer (background, non-blocking)
  - Raw PCM analysis pipe (`-f f32le`) for low-latency metrics
- Deterministic output naming under `shadow_out/`: `<basename>_<startms>_<endms>.wav`
- Persistent webview (wry/tao + WebView2) with simple UI and audio playback

### Repo layout
```
MPVShadow/
├─ mpv/
│  └─ scripts/
│     └─ analyzer_launcher.lua    # C key emits script-message to mpv IPC
├─ rust/
│  └─ shadow_analyzer/            # persistent analyzer + UI (Rust + wry)
│     ├─ assets/                  # index.html, style.css, script.js
│     └─ src/main.rs              # mpv IPC + ffmpeg + UI bridge
├─ shadow_out/                    # generated wav clips (auto-created)
└─ README.md
```

### Prerequisites
- Windows 10/11
- ffmpeg on PATH
- mpv
- Rust toolchain (`cargo`)
- Microsoft Edge WebView2 runtime (wry will use it on Windows)

### Setup
1) Enable mpv IPC (named pipe)
   - Add to your mpv config (e.g. `mpv/mpv.conf`):
     - `input-ipc-server=\\.\\pipe\\MPVShadow`
   - Or launch mpv with `--input-ipc-server=\\.\\pipe\\MPVShadow`.

2) Lua keybinding
   - Ensure `mpv/scripts/analyzer_launcher.lua` exists and binds C to:
     - `script-message cut_current_sub` (only when a subtitle is visible).

3) Build the analyzer
```bash
cd rust/shadow_analyzer
cargo build --release
```

### Run
1) Start mpv (with IPC enabled) and play a video with subtitles.
2) Run the analyzer binary:
```bash
rust/shadow_analyzer/target/release/shadow_analyzer.exe
```
3) Press C in mpv when a subtitle is visible.
   - Expected: OSD confirmation in mpv, a WAV in `shadow_out/`, console line like `first-byte latency: 55 ms; rms=... peak=...`, and the UI updates with a Play/Pause toggle for that WAV.

### Configuration (defaults are sane for now)
- Padding: 100 ms before/after the subtitle window
- Output: `shadow_out/` under current working directory
- Sample format: analysis stream `f32le`, WAV `pcm_s16le`, `48 kHz`, stereo
- Track select: uses absolute `ff_index` from mpv’s selected audio track

### Troubleshooting
- No pipe? Ensure mpv is started with `input-ipc-server=\\.\\pipe\\MPVShadow`.
- ffmpeg not found? Confirm `ffmpeg -version` works in a new terminal.
- UI window doesn’t open? Install the Evergreen WebView2 runtime.
- Access denied on rebuild (Windows): close the running `shadow_analyzer.exe` before `cargo build`.

### Roadmap
- Config file for padding, output dir, and fallbacks
- CSV log per cut (timestamp, path, window, subtitle text)
- Pitch tracking (F0) and basic high/low binning
- Optional switch back to in-process decode when dependencies stabilize

### License
MIT (see `LICENSE` if present). This is a work in progress—APIs may change.
