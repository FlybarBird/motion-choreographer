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

## Audio

Drop a WAV, AIFF or MP3 file on the window, or use **Import** in the **Media**
tab, then drag it onto the **Audio** lane. Drag the clip to move it and either
edge to trim it; the waveform shows what is left. The first seeded song,
Walk-In, carries a 12.4 second file with one click at 2.5 seconds: rehearse it
and the click is heard as the playhead crosses it.

The server plays the audio, not the app, so the sound comes out of the machine
the server runs on. While a song plays free-run, the playhead is the count of
samples the sound card has played, with the millisecond of jitter between its
callbacks filtered out, so picture and sound cannot drift apart.

Each output channel is routed in the Inspector (with nothing selected) to one of:

- **Program**: the mono mix of every unmuted clip.
- **LTC timecode**: the song's timecode, generated while it plays.
- **Silent**.

The default is Program on the left and Silent on the right. The transport bar
shows a meter for each channel and the output device. If no device can be
opened the server runs silent on the system clock and the bar says why.

```sh
ptz-server --list-audio            # device names
ptz-server --audio-device "USB"    # use the output whose name contains this
ptz-server --no-audio              # never open a device
ptz-server --selftest-audio        # one second of tone on the left, then the right
```

## Keys, the graph editor and Record

A preset move is one block. To shape a move by hand it becomes keys:

- Click the triangle on the **Camera move** lane to open the Pan, Tilt, Zoom,
  Focus and Elevation lanes. Each key is a diamond. Drag keys, drag a box round
  several, Delete removes them.
- Click a lane's name to select it; `K` adds a key at the playhead on the
  selected lanes (on every lane of the clip when none is selected).
- **Convert to keys** in the Inspector or the graph editor turns a preset move
  into keys that trace the same path. A keys clip can drive some axes and leave
  the rest to whatever came before it.

The **Graph editor** tab shows the selected clip as one curve per axis. It
shares the timeline's zoom, scroll and playhead. The ruler on the left measures
the axis whose chip is outlined; click a curve to switch.

| Do this | To |
| --- | --- |
| Drag a key | Change its time and value. Shift keeps it to one direction. |
| Drag a handle | Bend the curve. The opposite handle stays in line; hold Alt to break it. |
| Hold, Linear, Bezier | Choose how the curve leaves the selected keys. |
| Easy Ease, Ease In, Ease Out | Flatten the handles so the move starts or lands gently. |
| Double-click a curve | Add a key there without changing the shape. |
| `F` or **Frame** | Fit the view to the selected keys, or the clip. |
| **Speed** | Show how fast each axis moves. The amber line is the fastest the axis can go; anything above it is filled amber and named, and the clip turns amber on the timeline. |

The **Rig controller** tab drives the simulator by hand: drag the pad for pan
and tilt, the three sliders for zoom, focus and elevation. They spring back and
the rig stops when you let go. Click the pad to use the keys: arrows for pan and
tilt, `[` `]` zoom, `,` `.` elevation. **Top speed** scales all of it. **Store
preset** saves where the rig is now. If the app stops sending (a dropped
connection) the rig stops within a quarter of a second. Hand control is refused
while the rig is armed.

To record a move by hand:

1. Open the axis lanes and click the red dot on each lane Record should write.
   Pan and Tilt are on to start with.
2. Press **Record**, then play. The enabled axes follow the rig controller; the
   others keep playing the timeline.
3. Stop, Cue or **Record off** ends the pass. It lands on the timeline as a
   keys clip covering exactly the frames it was recorded on, replacing those
   axes over that range and nothing else. One undo removes the whole pass.

The pass is reduced to as few keys as reproduce it within 0.05 degrees of pan
and tilt, 0.2 % of zoom and focus and 1 mm of elevation; a stretch where the
rig stood still is two keys. The raw samples are kept beside the song
(`songs/<id>/passes/`), and **Fewer keys** / **More keys** in the Inspector fit
them again looser or tighter. Record works in Rehearse only.

## Lock and timecode

Free-run, the song plays on the sound card's clock. **Lock** makes it follow
timecode instead. The Lock button lists what it can follow:

- **LTC input**: timecode as audio on one channel of an input device.
- **MTC input**: MIDI timecode, quarter-frame and full-frame messages.
- **Art-Net timecode**: `ArtTimeCode` packets on UDP port 6454.
- **A file on the timeline**: any audio or video clip in the song.

With an input locked the main button reads **Play to code**. The transport waits
for code, takes four good frames to sync, then follows. If code drops out it
carries on for eight frames (**Freewheel** in the project settings) and then
waits again; if code jumps, it jumps with it. Code from before the song's first
frame or after its last holds the playhead at that end. Code at a different
frame rate from the project is refused, and the bar says which rate arrived.
Audio is sought to the code and pulled back in with a short crossfade whenever
it is more than a frame out.

The inputs are chosen in the Inspector with nothing selected, under **Timecode
inputs**: the LTC device and channel, the MIDI port, the Art-Net address to
listen on. They are devices on the server machine.

### Files that carry timecode

On import the server looks for the file's start timecode: the `bext` chunk of a
broadcast WAV, or the timecode track of a QuickTime or MP4 file. If there is
none, the Inspector takes a typed start, or **Read LTC from channel** decodes it
from one channel of the file itself. The media card shows `starts 01:00:10:00`
once a file has one.

A stamped file dropped on the timeline lands on its own timecode wherever the
pointer is; the ghost says so. Hold Alt to place it by hand instead. If its
timecode is outside the song, a dialog offers to move the song start to it. With
the clip selected the Inspector says whether it sits on its code and offers
**Move to its timecode**.

**Lock to this file** in the Inspector, or the file's entry in the Lock menu,
ties the transport to that clip: Play starts at the clip's first frame and stops
at its last, and Cue and the ruler still work inside it. A move recorded while
locked replays against the same code. An unstamped file locks by its position on
the timeline. Moving the clip off its code releases the lock and says why.

The third seeded song, Awards Bed, carries a stamped broadcast WAV sitting on
its code at `02:00:10:00`.

### Reference video

Video files go on the **Reference video** lane. Import makes a small JPEG of
every frame at the project rate, so the lane shows a filmstrip and the
**Reference monitor** tab shows the frame under the playhead. The sound of the
file is imported with it and shown under the filmstrip.

Video needs `ffmpeg` and `ffprobe` on the server machine (`brew install ffmpeg`).
Without them a video import is refused with that instruction; audio is not
affected.

## Transport

The transport bar always shows one main button, which is the next sensible step
(Rehearse, Play to code, Play Live, Stop, Reset E-Stop), and a one-line hint.
A disabled button says why when you hover it. **E-Stop** works from any station,
with or without the edit lease.

One station at a time holds the edit lease. Others watch, and can take over.

## Going live

Rehearse never sends a motion command to the rig. To move it:

1. **Arm (moves rig to playhead)**. The rig travels to the pose at the playhead
   at a fraction of its top speed (**Arm moves at** in the rig settings, half
   speed to start with). The bar reads ARMING, then ARMED when every axis has arrived.
2. **Play Live**, or **Play to code** when locked. Twenty times a second the
   server steers each axis toward the timeline and compares where the rig says
   it is with where the timeline was at the moment of that reading.
3. **Stop** holds the rig where it is and stays armed. **Disarm** returns to
   Rehearse.

Arm is refused, with the reason on the button, unless the transport is stopped,
Record is off, the loaded song is a committed revision with no unsaved edits,
every timeline issue is fixed (or, for speed and acceleration, accepted with
**Accept as programmed**), both devices are answering and the AutoPod is
calibrated. While armed the song, the presets and the project settings cannot
be changed.

The rig is held and output is cut, with the reason in red on the bar and in the
Rig controller tab until the next Arm or Play, when:

- an axis stays further from the timeline than its tolerance for longer than
  the tracking window (2 degrees of pan and tilt and half a second, unless the
  rig settings say otherwise);
- the next step would take an axis past its limit or break a keep-out rule;
- a device stops answering;
- the rig does not reach the playhead pose while arming.

**E-Stop** stops both devices, repeats the stop until they report standing
still, cuts output and latches until **Reset E-Stop**. It works from any
station, in any state, with or without the edit lease, and over the API as
`POST /api/v1/estop`.

While live the Rig controller tab shows how far each axis is from the timeline
against its tolerance.

## Limits and keep-out rules

Project settings (the Inspector with nothing selected) hold, per axis, the
range, top speed and top acceleration under **Rig limits**, and a
list of **Keep-out rules** of the form "when elevation is below 200 mm, tilt
must be above -10 degrees".

Both are checked twice. On the timeline, a clip that leaves the range or breaks
a rule turns amber, names the frame and the rule, and blocks Arm; these cannot
be accepted. Live, the pose about to be sent is checked every cycle, so a rule
that only the path to the playhead would break still holds the rig.

## The real rig

Under **Rig** in the project settings, **Drives** switches the server between
the simulator and **FR7 and AutoPod**. It can only be changed while stopped and
disarmed.

| Setting | What it is |
| --- | --- |
| FR7 address | VISCA over IP, UDP port 52381 unless given. |
| Web port, user, password | The camera's web interface, for preset names and pictures. |
| AutoPod address | The leg box, port 80 unless given. |
| Counts per degree, counts run backwards | How the head reports pan and tilt. |
| Speed steps | The speed of each of the 24 VISCA steps. Assumed until bring-up measures them. |
| Plain moves | Send a preset move as one absolute move and let the head run it, instead of steering it. |
| Query delay | How old a position is when it arrives. |
| Arm moves at, Tracking fault after, tolerances | As described under Going live. |
| AutoPod height map | Raw position against measured height, at three heights or more. **Add the current height** takes the raw value from the leg box. |

**Sync from rig** in the Presets tab reads the camera's presets. On the real rig
the camera has to be recalled to each one so its pose and picture can be read,
so the app asks first, and the sync is refused while armed or playing. E-Stop
abandons it.

To try all of this with no hardware, run stand-ins for both devices:

```sh
ptz-bringup fakes      # FR7 on 127.0.0.1:52381 (web 8088, login admin / fr7-password), AutoPod on 127.0.0.1:8089
```

and point the rig settings at them. They speak the same bytes as the adapters
expect, move at believable speeds and can be watched in that terminal.

### Bring-up on site

`ptz-bringup` talks to the two devices directly and measures what the project
needs. Stop the show server, or set its rig to the simulator, first.

```sh
ptz-bringup --fr7 192.168.0.100 --autopod 192.168.0.110 --user admin --password "..."
```

It runs seven steps, each of which says what it will move and asks before it
does, and brings the rig back to where it stood:

| Step | What it settles |
| --- | --- |
| `connect` | Both devices answer; the web login works; the AutoPod is calibrated. |
| `moves` | Which way the counts run on each axis, counts per degree, zoom and focus reach a position. |
| `timing` | How long a position query takes, how fast commands can be sent, the query delay. |
| `speeds` | The real speed of each VISCA step. |
| `tracking` | Steers a 10 degree eased pan and runs the same pan as a plain move, then decides between **velocity tracking** and **absolute moves**, and suggests a tolerance. |
| `autopod` | The height map (you measure the column at three heights), whether the speed byte is linear, top speed. |
| `estop` | How long each device takes to stand still after a stop, and how far it travels. |

`--only tracking,estop --from report.json` repeats chosen steps; `--quick`
shortens the runs. The report is written as JSON and printed in words. Then:

```sh
ptz-bringup --apply bringup-2026-11-14.json     # into the running server's project
```

Top speeds are only ever lowered by a report, and which rig the server drives
is never changed by it.

### What is assumed until bring-up confirms it

The adapters were written from Sony's published VISCA and CGI documents and the
community Companion module for the AutoPod, and tested against stand-ins built
from the same reading. None of it has met the real devices. Each of these is
measured or shown by a bring-up step and is a setting, not a constant:

- The speed of each VISCA pan and tilt step (assumed: 24 steps spread evenly
  by ratio from 0.02 to 60 degrees per second).
- 236 counts per degree, and which way the pan and tilt counts run.
- Zoom reported over 0 to 0x4000 and focus over 0 to 0xFFFF.
- Preset recall as `81 01 04 3F 02 pp FF` with `pp` one less than the preset
  number, and the shape of the web interface's answers (`PresetName=1,Wide,2,Lectern`).
- The VISCA sequence number after a reset (the link tries 1, then 0).
- The AutoPod commands `#PTS50tt` (speed), `#APC` (position) and `#RER`
  (calibrated), and that the speed byte `tt` is linear.

## The run log

Everything the server does to the show is written to `log/<date>.jsonl` in the
data folder and shown in the **Log** tab: each transport action and who asked
for it, each refusal with its reason, what was armed (song, revision, its
SHA-256, any accepted issues, the rig), the rig's pose and error once a second
while live, faults, how long an E-Stop took to bring the rig to a standstill,
devices connecting and dropping, preset syncs, lock and chase events.

Each record carries the hash of the one before it. `ptz-server --verify-log`
walks the chain and names the first record that was changed or removed.

## Checks

```sh
make test     # unit tests, API tests against a real server, crash test, UI tests
make check    # rustfmt, clippy with warnings as errors, the install script on stand-in binaries
make probe    # starts a server on a temp folder and tries every transport action in every state
```

`ptz-probe` can also be pointed at a running server (`ptz-probe --server URL`).
It refuses to run unless the rig is the simulator and the show is idle.

The tests that go live (`crates/server/tests/live.rs`, `hw.rs`, `bringup.rs`)
run the real server and adapters against the stand-in devices over loopback.

## Layout

| Path | What it is |
| --- | --- |
| `crates/core` | Song model, timecode, motion evaluation and feasibility, edit operations, transport rules, timecode codecs. No I/O. Linked by both programs so they cannot disagree. |
| `crates/server` | `ptz-server`, plus `ptz-probe` and `ptz-bringup`. |
| `crates/app` | `ptz-app`. |
| `scripts/macos-install.sh` | The Mac installer. |

Data folder (`~/PTZProgrammer` by default): `project.json`, `presets.json`,
`songs/<id>/working.json`, `draft.json`, `rev/NNNN.json` and `passes/`, `media/<id>/`
(the original file, the decoded audio, the waveform overview, the frame
pictures of a video), `log/`.
Every file is written to a temporary name and renamed, so a crash never leaves a
half-written song.
