# PTZ Director

Native Apple Silicon operator app + authoritative show server for arranging Sony ILME-FR7 + Polecam AutoPod compound PTZ/pedestal moves against song audio and SMPTE timecode, then playing the timeline.

This is a **from-scratch 0.2.0** architecture (Rust). The old 0.1.0 tree was used only as a product brief — none of its code was copied.

## Architecture

| Piece | Role |
|---|---|
| `ptz-core` | Shared domain: transport state machine, timecode (incl. 29.97DF + signed offsets), easing/interpolation, audio decode/waveform |
| `ptz-server` | Authoritative process: clock, transport, song revisions, hardware adapters, audio playback (cpal), REST `/api/v1` + WebSocket live feed |
| `ptz-client` | Native GPU UI (`eframe`/`egui`) — Premiere-style panels, AE-style timeline, program monitor |

Default backend: `http://127.0.0.1:8787`. The desktop remembers the URL. A workstation disconnect leaves a running show under server control.

### Transport: Preview vs live

- **Preview** (one click): plays timeline + audio + simulated program monitor. **Never** sets `hardware_live`. Physical FR7/AutoPod are not driven.
- **Live path** (deliberate): **Save/Commit → Prepare → Arm → Play**. Only after Arm does Play drive hardware adapters.
- **Stop** pauses; **Play** resumes; **Cue** re-cues to start; **E-Stop** is always available.

Button enablement in the UI is derived from the **same** `TransportPolicy` the server uses to accept/reject actions. Play cannot appear enabled while Editing (the old HTTP 409 bug class). Unavailable actions show the prerequisite next to the transport bar (e.g. “Unsaved changes — Save/Commit first”).

### Hardware

FR7 and AutoPod sit behind traits with **simulator** implementations used by default. Real VISCA / AutoPod adapters are explicitly `TODO(hardware)` — no unverified protocol claims.

Timecode sources: Internal (live), plus LTC / MTC / Art-Net adapters with parsing + signed offsets (Art-Net packet parse implemented; physical I/O stubs).

## Palette

`#091540` navy · `#f1f2ee` off-white · `#5aff15` green · `#4381c1` blue · `#262730` charcoal

## Build & run (dev)

```bash
make test          # fixtures + full workspace tests
make run-server    # http://127.0.0.1:8787
make run-client    # native window (needs display)
```

Demo songs seeded: **Walk-In** (SMPTE `01:00:00:00`, 25 fps), **Opening Number**, **Awards Bed**.  
Rig presets: **Stage Wide**, **Lectern MCU**, **Awards Two-Shot**.  
Audio routing demo: program mono → left, LTC generator → right (LTC gen initially **off**).

### Audio on a remote server

Playback is **server-side** via the machine’s default cpal output. For local Mac dev, run the server on the same Mac as the speakers. If the server is remote, audio emerges on the **server host**, not the operator laptop — use a local server for rehearsal audio, or route the server host into the show PA.

On macOS the server always tries Core Audio via cpal (the Linux `/dev/snd` heuristic is not used). Headless check without the GUI:

```bash
curl -s http://127.0.0.1:8787/api/v1/status | python3 -c \
  'import sys,json; a=json.load(sys.stdin)["audio_output"]; print(a)'
```

Expect `device` to be a real output name (not `silent`), plus `duration_ms`, `sample_rate`, `position_ms`, `transport_position_ms`, `drift_ms`, and `hardware_live`.

## macOS install (Apple Silicon)

On the Mac:

```bash
./scripts/macos-install.sh
```

This builds `aarch64-apple-darwin`, creates and ad-hoc signs `PTZ Director.app`, installs it to `~/Applications` (backing up any existing install), and packages `ptz-server` + `start-ptz-server.sh` beside it.

Then:

1. Run `~/Applications/PTZ Director Server/start-ptz-server.sh`
2. Open **PTZ Director**
3. Confirm backend `http://127.0.0.1:8787`

## Verify on a Mac

Fixture WAV: `fixtures/test-click-12_4s.wav` — **12.4 s**, distinctive transient click at **2.5 s**.

| Check | What you should see/hear | Needs |
|---|---|---|
| Timeline Preview | Playhead moves; program monitor framing pans/tilts/zooms/elevates with moves; audio plays (click at 2.5s) | Mac display + speakers; server local |
| Waveform | Imported clip shows peaks; click visible as a spike near 2.5s | Mac UI |
| Transport guidance | In Editing, **Play** disabled with “Save/Commit” hint; **Preview** enabled | Any |
| Live path | After Commit→Prepare→Arm, Play enables; `hardware_live=true` on status | Any (simulators) |
| E-Stop | Always available; latches and blocks Play/Preview until Clear | Any |
| Physical FR7 motion | — | Real ILME-FR7 + completed VISCA adapter (`TODO`) |
| Physical AutoPod | — | Real AutoPod + completed adapter (`TODO`) |
| LTC / MTC I/O | Art-Net parse unit-tested; physical LTC/MTC need hardware + I/O wiring (`TODO`) | LTC reader / MIDI / Art-Net source |

## Tests

```bash
make check   # fmt/clippy-or-check + test
make test
```

Coverage includes: transport transitions & rejections, preview vs live separation, E-Stop, timecode (29.97DF + signed offsets), audio decode/duration/waveform on the 12.4s fixture, interpolation/easing, API integration (409 on Play while Editing), UI-model button enablement ≡ server policy.

## Source artifact

```bash
make package   # → /opt/cursor/artifacts/ptz-director-0.2.0-src.tar.gz
```

Clean source only (no `target/`, caches, or databases).
