# PTZ Programmer

Timeline programming and playout for up to twelve Sony FR7 cameras, each on an
AutoPod+ pedestal.

You build a song by dragging camera presets onto a timeline. Each move takes its
time from the distance between the two poses and the rig's speed limits, and you
can stretch, squeeze, ease or key it by hand. Every camera has its own lane, its
own lens and its own place in a 3D model of the venue, so the monitors show what
each one will frame before anything moves. The show runs on a server; the
desktop app connects to it from the same Mac or from another machine.

Two programs:

- **`ptz-server`** owns the show: songs and their revisions, the transport, the
  clock, the rig, the run log. It keeps running when the app is closed.
- **`ptz-app`** is the operator's window: timeline, presets, monitors, inspector.
  It is a native app (Rust, `egui`), not a web page.

Until hardware is configured the server drives a built-in simulator rig, so
everything can be programmed and rehearsed with nothing plugged in.

Two longer documents go with this one:

- **[Setup Guide](docs/setup-guide.md)**: from a new machine to a system that is
  ready to program, including bringing up each real rig and a checklist for
  the first show.
- **[Programming Manual](docs/programming-manual.md)**: how to build, shape,
  rehearse and play a show, with a first-song walkthrough and a reference.

## Run it

Rust 1.95 or newer (`rustup update stable`).

```sh
make run-server     # data in ~/PTZProgrammer, listens on http://127.0.0.1:8787
make run-app        # in a second terminal
```

The app connects on launch to the server it last used, `http://127.0.0.1:8787`
the first time. If no server answers, the connect screen offers **Start a
server on this computer**: `ptz-server` with its defaults, which keeps running
after the app quits and drives whatever rig the project in `~/PTZProgrammer`
names.

A first run seeds three presets (Stage Wide, Lectern MCU, Awards Two-Shot) and
three songs (Walk-In, Opening Number, Awards Bed). `ptz-server --no-seed` starts
empty.

On Linux the app needs the usual X11 or Wayland libraries and the server needs
the ALSA headers (`libasound2-dev`).

### Install on a Mac

```sh
make install-mac
```

Builds for this Mac (Apple Silicon or Intel), assembles `PTZ Programmer.app`
with the three command-line tools inside it, signs it ad hoc, copies it to
`~/Applications` and links `ptz-server`, `ptz-probe` and `ptz-bringup` into
`~/.local/bin`. Running it again installs a newer version over the old one; it
asks you to quit the app and stop the server first. `make uninstall-mac`
removes the app and the links and leaves the show data alone.

The ad hoc signature is enough for the Mac that built it. A second Mac, for a
remote station, builds its own copy the same way.

### Installers

```sh
make package
```

Builds for the machine it is run on and writes two installers into `dist/`:

| | On a Mac | On Linux |
| --- | --- | --- |
| The app, with the show server as an option | `PTZ-Programmer-<version>-macos-<arch>.pkg` | `ptz-programmer-<version>-linux-<arch>.run` |
| The show server and tools alone | `PTZ-Server-<version>-macos-<arch>.pkg` | `ptz-server-<version>-linux-<arch>.run` |

The app installer always installs the app. **Show server and tools**
(`ptz-server`, `ptz-probe`, `ptz-bringup`) is a checkbox in the Mac installer,
ticked to start with, and a question in the Linux one (`--with-server` and
`--no-server` answer it in advance). Clear it on a station that only connects
to a server elsewhere; use the server installer on a machine that runs the
show without a screen.

- **Mac:** the app goes to `/Applications`, the server and tools to
  `/usr/local/ptz-programmer/bin`, which the installer puts on PATH for new
  terminals. The packages are not signed with a Developer ID unless
  `PTZ_INSTALLER_SIGN` names one, so on another Mac open one with a
  right-click and **Open**, or run `sudo installer -pkg <file> -target /`.
- **Linux:** `sh ptz-programmer-<version>-linux-<arch>.run` installs under
  `~/.local` (or `--prefix`), with an entry in the applications menu.
  `--uninstall` removes what it installed.

An app installed without the server greys out **Start a server on this
computer** and says why.

A Mac installer has to be built on a Mac: macOS programs cannot be compiled
anywhere else. An Apple Silicon Mac builds the Intel pair as well:

```sh
make package                                    # for this Mac
PTZ_TARGET=x86_64-apple-darwin make package     # for Intel Macs, from an Apple Silicon one
```

### A remote operator station

```sh
ptz-server --bind 0.0.0.0:8787 --access-key "choose-a-key"
```

The server refuses to listen beyond this machine without a key. Enter the
server address and the key on the app's connect screen.

## Building a song

1. **New** in the header. Name the song and give it the timecode its first frame
   plays at, for example `01:00:00:00`.
2. Drag a preset from the **Presets** tab onto the timeline. It lands on the
   **Camera move** lane at the pointer's time, whichever lane the pointer is
   over. The ghost shows where it lands and how long the move takes: `Stage
   Wide to Lectern MCU, 7.9 s`. A move starts from wherever the camera already is.
3. Drag a move to slide it. Drag its right edge to make it longer; it switches
   from auto time to custom time. Squeeze it too short and it turns amber with
   the reason; **Use auto time** in the Inspector clears it.
4. Press **Space** to rehearse. The Program monitor shows the selected camera's
   framing through its lens; nothing is sent to the rig.
5. **Save**, then **Commit**. Only a committed revision can be armed.

Edges snap to the playhead, markers and other clips; hold Shift to turn snapping
off. Cmd-Z and Shift-Cmd-Z undo and redo. `M` adds a marker, `\` fits the song
to the window, Home cues to the start, the arrow keys step one frame.

## Cameras

A show holds one to twelve cameras. The camera bar under the transport lists
them; click one, or press `1` to `9`, `0`, `-` and `=`, to select it. The
Program monitor, the Presets tab, the Rig controller and Record all follow the
selected camera. Each camera has its own **Camera move** lane on the timeline,
with its own axis lanes. A preset belongs to one camera and always lands on
that camera's lane, whichever lane the pointer is over. A move can be dragged
to another camera's lane afterwards.

Cameras are added, named, reordered and removed in the Inspector with nothing
selected, under **Cameras**. **Add a camera** copies the selected camera's
lens, limits and rules onto a new one on the simulator, two metres to its
right. Removing a camera removes its presets, and its clips block Arm until
they are deleted. Rig limits, keep-out rules, the rig settings, the lens and
the position below it are those of the selected camera.

**When a camera faults** chooses what the others do: *Hold that camera, the
others carry on* (the default), or hold them all. E-Stop always stops every
camera.

The **Multiview** tab shows every camera's picture at once, in a grid that
grows from one tile to four by three, with each camera's name, state and focal
length. Click a tile to select that camera.

### Lenses

Each camera has a lens: its name, the sensor width (full frame, Super 35 or a
typed width), the focal length at the wide and long ends, and the widest
aperture. The list offers the FR7 kit zoom (FE PZ 28-135mm F4 G OSS) and four
other E-mount zooms; any other lens is typed in.

From those the Inspector shows, for the zoom the camera is at now: the focal
length, the angle of view (wide, high and diagonal), how far away the centre of
the frame lands in the stage model, how wide and high the frame is there, and
the range that is sharp at that distance. Tick **At a distance of** to work
those out for a typed distance instead. Every picture in the app (Program,
Multiview, the preset cards, the view cones in the Stage view) is drawn with
these numbers, so changing a lens changes the framing everywhere.

Zoom is taken to run evenly in stops from the wide end to the long end.
Bring-up can check that against the real lens.

### The stage model

The Program monitor is a rendering of the venue from where the camera stands.
Until a model is imported it shows a small built-in stage. To use your own:

- Drop an `.obj` or `.3ds` file on the window, or use **Import a model** in the
  Inspector under **Stage** (or type its path in the field beside it).
- An OBJ's `.mtl` file is read from the same folder for its colours. Textures
  are not used; each surface takes its material's colour.
- The app reads the file and sends the server a compact mesh, so every station
  sees the same model. Up to 1.5 million triangles and 200 MB.

The Inspector then shows the model's size in metres. If it is wrong, change
**One unit is** (millimetres, centimetres, metres, inches, feet; the first guess
comes from the model's size) or **Up** (OBJ files are taken as Y up, 3DS as Z
up). **Turned** and **Moved** place it; **Sit on the floor** puts its lowest
point at height zero and **Centre** puts its middle over the origin. **Remove
the model** brings the built-in stage back for every station.

Distances are in metres, Z is up, and a camera with heading 0 and pan 0 looks
along +Y.

### Placing cameras: the Stage view

The **Stage** tab shows the model from outside with every camera, its name and
the cone it sees, which follows its pan, tilt, zoom and lens as the song plays.

| Tool | What it does |
| --- | --- |
| **Select** | Click a camera, or its name, to select it. Drag empty space to look around. |
| **Move** | Drag a camera across the floor. With **Snap to model** it follows the surface under the pointer, so it can stand on a riser or a balcony. |
| **Rotate** | Drag around a camera to turn where its pan 0 points. Shift steps by 5 degrees. |
| **Place** | Click where the selected camera should stand. |
| **Aim** | Click what the selected camera's pan 0 should face. |
| **Measure** | Click two points; the distance, and how much of it is along the floor and up, is read out. |

**Perspective**, **Top**, **Front** and **Side** choose the angle and fit the
stage to it; **Fit** does so again. In the flat views **Cut** hides a ceiling or
a near wall. Right-drag or Shift-drag slides the view and the scroll wheel
zooms. Escape cancels a drag, and **Undo move** puts the last camera moved back.

The same numbers are in the Inspector under **Position on the stage**: floor
position, heading, and the height of the lens above the floor with the column
down. **Face the stage** turns the camera to the middle of the model and **Drop
to the surface below** stands it on whatever is under it. Placing cameras needs
the edit lease and is refused while armed.

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

Playing, scrubbing and cueing in Rehearse never send a motion command to the
rig. (**Sync from rig** does move a real camera, and asks first.) To move it:

1. **Arm (moves rig to playhead)**. Every camera's rig travels to its pose at
   the playhead at a fraction of its top speed (**Arm moves at** in the rig
   settings, half speed to start with). The bar reads ARMING, then ARMED when
   every axis of every camera has arrived.
2. **Play Live**, or **Play to code** when locked. Twenty times a second the
   server steers each axis toward the timeline and compares where the rig says
   it is with where the timeline was at the moment of that reading.
3. **Stop** holds the rig where it is and stays armed. **Disarm** returns to
   Rehearse.

Arm is refused, with the reason on the button, unless the transport is stopped,
Record is off, the loaded song is a committed revision with no unsaved edits,
every timeline issue is fixed (or, for speed and acceleration, accepted with
**Accept as programmed**), and every camera's devices are answering with its
AutoPod+ calibrated. While armed the song and the project settings cannot be
changed. Presets can still be stored and deleted: a move keeps its own copy of
the pose it was made from, so the armed song is not affected.

While armed and stopped, **Cue**, Home, a click on the ruler, or a jump in the
timecode the show is locked to sends every rig to the pose at the new
playhead, at the Arm speed; the bar reads ARMING until they are there.

A camera's rig is held and its output is cut, with the reason in red on the bar and in the
Rig controller tab, until the show is disarmed and armed again (Play does not
bring a held camera back), when:

- an axis stays further from the timeline than its tolerance for longer than
  the tracking window (2 degrees of pan and tilt and half a second, unless the
  rig settings say otherwise);
- the next step would take an axis past its limit or break a keep-out rule;
- a device stops answering;
- the rig does not reach the playhead pose while arming.

**E-Stop** stops every camera and pedestal, repeats the stop until they report standing
still, cuts output and latches until **Reset E-Stop**. It works from any
station, in any state, with or without the edit lease, and over the API as
`POST /api/v1/estop`.

Told to stop while live (Ctrl-C, `kill`, a closed terminal, a system
shutdown), the server stops every rig and waits for it to stand still before
it exits. Killed outright, crashed or without power it cannot: a rig keeps its
last speed command until its own limits end the move. For that case the server
sends each camera and pedestal a stop as soon as it connects to it, so
starting the server again stops a rig that was left moving.

E-Stop is a software stop: a command sent over the network by a running
server. It does not replace the AutoPod+'s own stop control or cutting power,
and it cannot work if the server or the network is down. Only one controller
should drive a camera at a time; a second one sending to the same FR7 will
fight this one.

Going live switches the FR7's focus to manual so the timeline can drive it.
Nothing switches it back; set it at the camera if you want autofocus after the
show. A camera set to **FR7 and AutoPod+** with the AutoPod+ address left
empty runs without a pedestal: its elevation is taken to be wherever the
timeline says.

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

Under **Rig** in the project settings, **Drives** switches the selected camera
between the simulator and **FR7 and AutoPod+**. Each camera has its own
addresses and settings, and a show can mix real rigs and simulated ones. It can
only be changed while stopped and disarmed.

| Setting | What it is |
| --- | --- |
| FR7 address | VISCA over IP, UDP port 52381 unless given. |
| Web port, user, password | The camera's web interface, for preset names and pictures. |
| AutoPod+ address | The leg box, port 80 unless given. |
| Counts per degree, counts run backwards | How the head reports pan and tilt. |
| Speed steps | The speed of each of the 24 VISCA steps. Assumed until bring-up measures them. |
| Plain moves | Send a preset move as one absolute move and let the head run it, instead of steering it. |
| Query delay | How old a position is when it arrives. |
| Arm moves at, Tracking fault after, tolerances | As described under Going live. |
| AutoPod+ height map | Raw position against measured height, at three heights or more. **Add the current height** takes the raw value from the leg box. |

**Sync from rig** in the Presets tab reads the camera's presets. On the real rig
the camera has to be recalled to each one so its pose and picture can be read,
so the app asks first, and the sync is refused while armed or playing. E-Stop
abandons it.

To try all of this with no hardware, run stand-ins for both devices:

```sh
ptz-bringup fakes      # FR7 on 127.0.0.1:52381 (web 8088, login admin / fr7-password), AutoPod+ on 127.0.0.1:8089
```

and point the rig settings at them. They speak the same bytes as the adapters
expect, move at believable speeds and can be watched in that terminal.
`ptz-bringup fakes --count 12` runs twelve pairs, each on the next ports up
(VISCA 52381 to 52392, web 8088, 8090, ..., AutoPod+ 8089, 8091, ...).

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
| `connect` | Both devices answer; the web login works; the AutoPod+ names its model and firmware, is switched on and is calibrated. |
| `moves` | Which way the counts run on each axis, counts per degree, zoom and focus reach a position. |
| `timing` | How long a position query takes, how fast commands can be sent, the query delay. |
| `speeds` | The real speed of each VISCA step. |
| `tracking` | Steers a 10 degree eased pan and runs the same pan as a plain move, then decides between **velocity tracking** and **absolute moves**, and suggests a tolerance. |
| `autopod` | The height map (you measure the column at three heights), whether the speed byte is linear, top speed. |
| `estop` | How long each device takes to stand still after a stop, and how far it travels. |

`--only tracking,estop --from report.json` repeats chosen steps; `--quick`
shortens the runs; `--yes` answers every prompt and skips the height map, which
needs someone to measure. The report is written as JSON and printed in words.
Then:

```sh
ptz-bringup --apply bringup-2026-11-14.json --camera 3    # into that camera of the running server's project
```

Without `--camera` (a number in the camera list, or an id) the values go to
camera 1.

Top speeds are only ever lowered by a report, and which rig the server drives
is never changed by it.

### What is assumed until bring-up confirms it

The FR7 adapter was written from Sony's published VISCA and CGI documents and
tested against a stand-in built from the same reading. It has not met the real
camera. Each of these is measured or shown by a bring-up step and is a setting,
not a constant:

- The speed of each VISCA pan and tilt step (assumed: 24 steps spread evenly
  by ratio from 0.02 to 60 degrees per second).
- 236 counts per degree, and which way the pan and tilt counts run.
- Zoom reported over 0 to 0x4000 and focus over 0 to 0xFFFF.
- Preset recall as `81 01 04 3F 02 pp FF` with `pp` one less than the preset
  number, and the shape of the web interface's answers (`PresetName=1,Wide,2,Lectern`).
- The VISCA sequence number after a reset (the link tries 1, then 0).

The AutoPod+ adapter follows the exchange recorded between a real hand
controller (TX v3.30) and leg box (RX v3.49e) in the `autopod-osc-bridge`
project, and the stand-in answers the way that leg box does:

| Known from those recordings | Still measured on site |
| --- | --- |
| A request is `GET /cgi-bin/aw_ptz?cmd=%23APC&res=1 HTTP/1.0` with no headers; the reply is HTTP/1.0, closed after the body. | That the column moves at a speed proportional to the speed byte. |
| `#APC` answers `aPC` and two four-digit hex fields; elevation is the second. `#RER` answers `rER00` when healthy. | How raw position maps to millimetres (the height map). |
| `#PTS50tt` answers `pTS50tt`; `tt` runs from 00 (full down) through 50 (rest) to 99 (full up). | Top speed up and down. |
| `QID` answers `OID:AUTOPOD+`, `#V?` the firmware, `#O` `p1` or `p0` (standby). | How long a stop takes and how far the column travels. |
| A command the unit does not have answers `ER1:` and the command. `/cgi-bin/get_state` is 404. | |

The server reads the model, firmware and power state when it connects and shows
them with the adapter ("AutoPod+ V3.49e at 192.168.60.204:80 answers in 3 ms").
A unit in standby blocks Arm; the show never switches it on by itself. A
refused command is reported as a refusal, not as a lost connection.

## The run log

Everything the server does to the show is written to `log/<date>.jsonl` in the
data folder and shown in the **Log** tab: each transport action and who asked
for it, each refusal with its reason, what was armed (song, revision, its
SHA-256, any accepted issues, the rig), the rig's pose and error once a second
while live, faults, how long an E-Stop took to bring the rig to a standstill,
devices connecting and dropping, preset syncs, lock and chase events.

Each record carries the hash of the one before it. `ptz-server --verify-log`
walks the chain and names the first record that was changed, or removed from
the middle. Records cut off the end of the newest file cannot be detected this
way.

## Checks

```sh
make test     # unit tests, API tests against a real server, crash test, UI tests
make check    # rustfmt, clippy with warnings as errors, the install script on stand-in binaries
make probe    # starts a server on a temp folder and tries every transport action in every state it can reach without timecode
```

`ptz-probe` can also be pointed at a running server (`ptz-probe --server URL`).
It refuses to run unless the rig is the simulator and the show is idle. It
takes the edit lease from whoever holds it, presses E-Stop, and slows the Arm
speed for the run, putting it back at the end; if it is killed part-way, a
"Probe Scratch" song and the slowed Arm speed are left behind. Use it on a
scratch data folder, or before real work starts.

The tests that go live (`crates/server/tests/live.rs`, `hw.rs`, `bringup.rs`)
run the real server and adapters against the stand-in devices over loopback.

## Layout

| Path | What it is |
| --- | --- |
| `crates/core` | Song model, timecode, motion evaluation and feasibility, edit operations, transport rules, timecode codecs. No I/O. Linked by both programs so they cannot disagree. |
| `crates/scene` | Reads OBJ and 3DS files, holds the mesh, renders it with a depth buffer on the CPU, and finds what a ray hits. No dependencies. |
| `crates/server` | `ptz-server`, plus `ptz-probe` and `ptz-bringup`. |
| `crates/app` | `ptz-app`. |
| `scripts/macos-install.sh` | The Mac installer. |

Data folder (`~/PTZProgrammer` by default): `project.json`, `presets.json`,
`state.json` (which song is open),
`songs/<id>/working.json`, `draft.json`, `rev/NNNN.json` and `passes/`, `media/<id>/`
(the original file, the decoded audio, the waveform overview, the frame
pictures of a video), `stage/model.ptzmesh` (the imported stage model), `log/`.
Every file is written to a temporary name and renamed, so a crash never leaves a
half-written song.

The server and the app talk plain HTTP, the access key is kept in the app's
settings file, and the FR7 web password is kept in `project.json` and sent to
every connected station. Run the show on a closed production network. The
server also takes its key from the `PTZ_ACCESS_KEY` environment variable, and
`PTZ_APP_DIR` moves the app's settings file.

`LICENSE` is the MIT licence this is released under; `THIRD-PARTY-NOTICES.md`
lists the fonts and crates built into the app.
