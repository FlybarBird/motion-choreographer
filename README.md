# PTZ Programmer

Timeline programming and playout for a Sony FR7 on an AutoPod pedestal.

You build a song by dragging camera presets onto a timeline. Each move takes its
time from the distance between the two poses and the rig's speed limits, and you
can stretch, squeeze, ease or key it by hand. The show runs on a server; the
desktop app connects to it from the same Mac or from another machine.

Two programs:

- **`ptz-server`** owns the show: songs and their revisions, the transport, the
  clock, the rig, the run log. It keeps running when the app is closed.
- **`ptz-app`** is the operator's window: timeline, presets, monitors, inspector.
  It is a native app (Rust, `egui`), not a web page.

Until hardware is configured the server drives a built-in simulator rig, so
everything can be programmed and rehearsed with nothing plugged in.

## Run it

Rust 1.88 or newer.

```sh
make run-server     # data in ~/PTZProgrammer, listens on http://127.0.0.1:8787
make run-app        # in a second terminal
```

The app connects to `http://127.0.0.1:8787` on launch. If no server is running,
the connect screen offers **Start a server on this computer**.

A first run seeds three presets (Stage Wide, Lectern MCU, Awards Two-Shot) and
three songs (Walk-In, Opening Number, Awards Bed). `ptz-server --no-seed` starts
empty.

On Linux the app needs the usual X11 or Wayland libraries and the server needs
the ALSA headers (`libasound2-dev`).

### Install on an Apple Silicon Mac

```sh
make install-mac
```

Builds for `aarch64-apple-darwin`, assembles `PTZ Programmer.app` with the three
command-line tools inside it, signs it ad hoc, copies it to `~/Applications` and
links `ptz-server`, `ptz-probe` and `ptz-bringup` into `~/.local/bin`.

### A remote operator station

```sh
ptz-server --bind 0.0.0.0:8787 --access-key "choose-a-key"
```

The server refuses to listen beyond this machine without a key. Enter the
server address and the key on the app's connect screen.

## Building a song

1. **New** in the header. Name the song and give it the timecode its first frame
   plays at, for example `01:00:00:00`.
2. Drag a preset from the **Presets** tab onto the **Camera move** lane. The
   ghost shows where it lands and how long the move takes: `Stage Wide to
   Lectern MCU, 7.9 s`. A move starts from wherever the camera already is.
3. Drag a move to slide it. Drag its right edge to make it longer; it switches
   from auto time to custom time. Squeeze it too short and it turns amber with
   the reason; **Use auto time** in the Inspector clears it.
4. Press **Space** to rehearse. The Program monitor shows the framing; nothing is
   sent to the rig.
5. **Save**, then **Commit**. Only a committed revision can be armed.

Edges snap to the playhead, markers and other clips; hold Shift to turn snapping
off. Cmd-Z and Shift-Cmd-Z undo and redo. `M` adds a marker, `\` fits the song
to the window, Home cues to the start, the arrow keys step one frame.

## Transport

The transport bar always shows one main button, which is the next sensible step
(Rehearse, Play to code, Play Live, Stop, Reset E-Stop), and a one-line hint.
A disabled button says why when you hover it. **E-Stop** works from any station,
with or without the edit lease.

One station at a time holds the edit lease. Others watch, and can take over.

## Checks

```sh
make test     # unit tests, API tests against a real server, crash test, UI tests
make check    # rustfmt, clippy with warnings as errors, the install script on stand-in binaries
make probe    # starts a server on a temp folder and tries every transport action in every state
```

`ptz-probe` can also be pointed at a running server (`ptz-probe --server URL`).
It refuses to run unless the rig is the simulator and the show is idle.

`ptz-server --verify-log` checks the hash chain of the run log.

## Layout

| Path | What it is |
| --- | --- |
| `crates/core` | Song model, timecode, motion evaluation and feasibility, edit operations, transport rules, timecode codecs. No I/O. Linked by both programs so they cannot disagree. |
| `crates/server` | `ptz-server`, plus `ptz-probe` and `ptz-bringup`. |
| `crates/app` | `ptz-app`. |
| `scripts/macos-install.sh` | The Mac installer. |

Data folder (`~/PTZProgrammer` by default): `project.json`, `presets.json`,
`songs/<id>/working.json`, `draft.json` and `rev/NNNN.json`, `media/`, `log/`.
Every file is written to a temporary name and renamed, so a crash never leaves a
half-written song.
