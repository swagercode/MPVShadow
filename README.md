## mpv-shadow-coach

Shadow subtitle lines in mpv: press Ctrl+R to jump to the current subtitle's start, record your mic while the line plays, run an analyzer, then review everything in a live dark-mode analysis window. Re-press Ctrl+R to redo the same line—the window simply updates.

### Features
- Single hotkey loop: seek → play → record → analyze → resume
- Webview (Rust + Chart.js) window with dual waveform+pitch charts for mic and source
- Space bar plays both tracks in sync; per-track play/stop buttons
- JSON log per take (`shadow_out/shadow_log.csv`) with match %, timing, speed, and pitch metrics
- Optional Whisper-tiny alignment for more detailed timings

### Repo layout
```
mpv-shadow-coach/
├─ mpv/
│  └─ scripts/
│     └─ shadow.lua          # hotkeys + orchestration
├─ python/
│  └─ shadow_analyze.py      # librosa + faster-whisper metrics pipeline
├─ rust/
│  └─ shadow_viz/            # long-lived webview window (wry)
├─ shadow_out/               # generated wav slices, viz_state.json, CSV log (auto-created)
├─ requirements.txt
└─ README.md
```

### Prereqs
- Python 3.9+
- Rust toolchain (`cargo`) for the visualizer
- ffmpeg available in your PATH
- mpv

### Setup
1. Install Python dependencies (venv recommended)
   ```bash
   pip install -r requirements.txt
   ```
2. Build the Rust visualizer once
   ```bash
   cd rust/shadow_viz
   cargo build --release
   ```
   The Lua script expects the binary at `rust/shadow_viz/target/release/shadow_viz.exe`. Adjust `options.viz_exe` in `shadow.lua` if you move it.
3. Configure your mic device in `mpv/scripts/shadow.lua`:
   - Windows (`dshow`): list devices with `ffmpeg -hide_banner -f dshow -list_devices true -i dummy`
   - macOS (`avfoundation`): use indexes like `:0`, `:1`, …
   - Linux (`pulse`): `default` or the specific `alsa_input.*` name

### Run
```bash
mpv --script=./mpv/scripts/shadow.lua yourfile.mkv
```
While a subtitle is visible, hit **Ctrl+R**:
1. mpv seeks to the subtitle start and begins playback
2. ffmpeg records your mic for the subtitle’s duration
3. Playback pauses briefly while Python analyzes timing, speed, and pitch
4. The Rust window updates in-place (wave + pitch charts, stats, and playback controls)
5. CSV log is appended in `shadow_out/shadow_log.csv`

Press **Ctrl+R** again anytime to redo the current (or most recent) line—the window refreshes without spawning a new one. Press **Ctrl+A** to toggle auto mode (experimental) that walks subtitles automatically.

### Analysis window controls
- Line text and metrics at the top (dark theme)
- Mic chart (waveform + pitch overlay) and Source chart stacked vertically
- `Play mic` / `Play source` / `Stop` buttons control individual tracks
- `Space` or the **Play both** button resets both tracks to 0 and plays A/B in sync

### Notes
- Lua uses mpv properties `sub-start` / `sub-end`; no polling loop required
- Whisper is optional (`options.whisper = true`) and downloads the tiny model on first run
- The visualizer watches `shadow_out/viz_state.json`; delete the file or close the window if you need to reset—`shadow.lua` will respawn it automatically on the next Ctrl+R
- WAV slices (`*-src.wav`, `*-mic.wav`) are kept in `shadow_out/` for quick review or archival


