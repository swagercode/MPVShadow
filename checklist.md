Immediate checklist (no code): Wire C key to a persistent Rust analyzer via mpv IPC

1) Enable mpv IPC (named pipe on Windows)
   - Create or edit `mpv/mpv.conf` (or your user config) and add:
     - `input-ipc-server=\\.\pipe\MPVShadow`
   - Alternatively, launch mpv with `--input-ipc-server=\\.\pipe\MPVShadow`.

2) Confirm your C keybinding exists in Lua
   - In `mpv/scripts/analyzer_launcher.lua`, keep your C handler that triggers the action.
   - It should run only when a subtitle is visible; otherwise show a short OSD and return.

3) Decide the trigger message name
   - Use: `cut_current_sub`.
   - On C, your Lua should emit a `script-message cut_current_sub` (no networking in Lua).

4) Rust analyzer responsibilities (persistent app)
   - On startup: connect to `\\.\pipe\MPVShadow` (mpv JSON IPC) and subscribe to events.
   - Listen for `client-message`/`script-message` events with name `cut_current_sub`.
   - When received, query mpv properties via IPC:
     - `sub-text`, `sub-start`, `sub-end`, `sub-delay`, `time-pos`, `duration`, `path`, `track-list`.
   - From `track-list`, select the audio track where `type == "audio" && selected == true` and read its `ff-index` (if present), plus `id/lang` for logging.
   - Compute the cut window:
     - pad = 0.10 s; `start_s = clamp(sub-start - pad, 0, duration)`; `end_s = clamp(sub-end + pad, 0, duration)`; require `start_s < end_s`.
   - Fast path (analysis): seek, decode [start_s, end_s], compute simple metrics (RMS, peak), and immediately send a short OSD back to mpv via `show-text`.
   - Background (optional): write the clip to `shadow_out` (WAV/FLAC) without blocking the fast path.

5) Output conventions (to keep things tidy)
   - Directory: `shadow_out`.
   - Filename: `<basename>_<startms>_<endms>.wav` (deterministic and sortable).
   - Log (optional): append a CSV row with `ts,event,media_path,clip_path,start_s,end_s,subtitle_text`.

6) Sanity checks to run right now
   - Start mpv and verify the named pipe exists (mpv creates `\\.\pipe\MPVShadow`).
   - Launch your Rust analyzer (release build recommended) and ensure it connects to the pipe.
   - Play a file with subtitles visible; press C and confirm the analyzer receives the trigger.
   - Verify a clip appears in `shadow_out` and an OSD confirmation shows in mpv.

Notes
   - This path avoids any Lua networking or extra helper processes; the analyzer pulls everything it needs from mpv via IPC after your C key trigger.
   - If you later prefer a push model, you can switch to a tiny client that forwards JSON to your analyzer; not needed for the current plan.


Done so far
- [x] Enabled mpv IPC via `input-ipc-server=\\.\pipe\MPVShadow` and verified with PowerShell
- [x] Added Lua keybind (C) to emit `script-message cut_current_sub`
- [x] Rust analyzer connects to mpv pipe, subscribes to `client-message`, and catches `cut_current_sub`
- [x] Queries properties: `sub-text`, `sub-start`, `sub-end`, `duration`, `path`, `track-list`
- [x] Computes padded/clamped `[start_s, end_s]` and finds selected audio `ff-index`
- [x] Fixed mapping to absolute stream index (`-map 0:{ff_index}`) so JP audio is used
- [x] Deterministic output path under `shadow_out` using `<basename>_<startms>_<endms>.wav`
- [x] OSD confirmation with chosen window and track info
- [x] Proved the end-to-end cut works (external ffmpeg to WAV)

In progress / decisions
- [x] Target latency < 300 ms (keep analyzer persistent; minimize per‑press work)
- [ ] Use external ffmpeg for buffered PCM stream (`pipe:1`) to avoid native linking for now
- [ ] Pause in‑process decode via ffmpeg-next; native linking is blocked by FFmpeg 6.1 API changes

Next up (implementation tasks)
- [ ] Revert file‑write path to: spawn external ffmpeg to WAV (background, don’t block)
- [ ] Add second external ffmpeg that outputs `-f f32le -ar 48000 -ac 2 pipe:1`; read stdout in Rust and compute metrics
- [ ] Make WAV write fully asynchronous relative to analysis (no join; surface result immediately)
- [ ] Add robust process/error handling and timeouts for both ffmpeg processes
- [ ] Centralize config (padding ms, sample rate/channels, output dir, language fallback if `ff_index` missing)
- [ ] Write minimal CSV log for each cut (`ts,event,media,clip,start,end,lang`)
- [ ] Unit‑test helpers (path/filename builder, padding/clamp, track select) where feasible

Future (after buffered stream works)
- [ ] Implement pitch tracking (F0) and simple High/Low binning with hysteresis (no full accent analysis)
- [ ] Optional: switch back to single in‑process decode when a crate compatible with your FFmpeg is pinned
- [ ] Optional: SIMD optimize hot loops only if profiling shows need