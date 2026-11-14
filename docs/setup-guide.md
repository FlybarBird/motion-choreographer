# PTZ Programmer: Setup Guide

This guide takes a new installation from nothing to a system that is ready to
program: software installed, the show server running, the project describing
your cameras, lenses and venue, sound and timecode connected, and each real rig
measured and proven safe. Programming itself is covered in the
[Programming Manual](programming-manual.md).

Do the parts in order. Parts 1 to 6 need no hardware at all; everything can be
set up and rehearsed on the built-in simulator, and you can stop there until the
rig arrives.

| Part | What it gets you |
| --- | --- |
| [1. What you need](#1-what-you-need) | The shopping and cabling list |
| [2. Install](#2-install) | The app and the tools on your machine |
| [3. Start the server and connect](#3-start-the-server-and-connect) | A running show you can open |
| [4. The project: frame rate and cameras](#4-the-project-frame-rate-and-cameras) | One to twelve cameras with the right lenses |
| [5. The venue: stage model and camera positions](#5-the-venue-stage-model-and-camera-positions) | Monitors that show what each camera will frame |
| [6. Sound and timecode](#6-sound-and-timecode) | Audio out, LTC out, and what Lock listens to |
| [7. The real rig](#7-the-real-rig) | Each FR7 and AutoPod+ connected, measured and limited |
| [8. Operator stations](#8-operator-stations) | A second machine, the edit lease |
| [9. Before the first show](#9-before-the-first-show) | The checklist |
| [10. Looking after it](#10-looking-after-it) | Backups, updates, the run log |
| [11. When something is wrong](#11-when-something-is-wrong) | Messages and what to do |

---

## 1. What you need

### Computers

- **A server machine.** It runs `ptz-server`, which owns the show, plays the
  audio and talks to the cameras. It stays on for the whole show. An Apple
  Silicon Mac is the installed target; Linux also works when built from source.
- **An operator station.** It runs the app. It can be the same machine as the
  server, which is the simplest arrangement, or another machine on the network.

### For each camera position (one to twelve)

| Item | What the software uses |
| --- | --- |
| Sony FR7 | VISCA over IP on UDP port 52381 for motion, and the camera's web interface for preset names and pictures |
| AutoPod+ pedestal | The leg box's network interface on port 80, for elevation |
| The lens on that FR7 | Its focal range, widest aperture and the sensor width in use |

You will need each device's IP address, and the web interface user name and
password of each FR7.

### Network

Put the server machine, every FR7 and every AutoPod+ leg box on one wired
network, with fixed addresses. Operator stations reach the server on TCP port
8787. Wi-Fi between the server and the rig is a risk you do not need to take:
the live loop sends to every camera twenty times a second.

| Traffic | From | To | Port |
| --- | --- | --- | --- |
| App to server | operator station | server | TCP 8787 |
| Camera motion | server | each FR7 | UDP 52381 |
| Preset names and pictures | server | each FR7 | TCP 80 (or the web port you set) |
| Elevation | server | each AutoPod+ | TCP 80 |
| Art-Net timecode, if used | timecode source | server | UDP 6454 |

### Sound and timecode (as your show needs)

- An audio output on the server machine for the song's audio. A stereo
  interface gives you the mix on one channel and LTC timecode on the other.
- An audio input on the server machine if the show follows LTC.
- A MIDI interface on the server machine if the show follows MTC.
- `ffmpeg` and `ffprobe` on the server machine if you want reference video
  (`brew install ffmpeg`). Audio does not need them.

### To build the software

- Rust 1.88 or newer (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).
- On a Mac, the Xcode command line tools (`xcode-select --install`).
- On Linux, the ALSA headers (`libasound2-dev`) and the usual X11 or Wayland
  libraries.

---

## 2. Install

### On an Apple Silicon Mac

From the project folder:

```sh
make install-mac
```

The first build takes several minutes. When it finishes you have:

- `~/Applications/PTZ Programmer.app`, the desktop app, with the command-line
  tools inside it;
- `ptz-server`, `ptz-probe` and `ptz-bringup` linked into `~/.local/bin`.

If the installer says `~/.local/bin` is not on your PATH, run the line it
prints, then open a new terminal.

The app is signed ad hoc, which is enough to run it on the Mac that built it.
macOS may ask for microphone access the first time the server listens for LTC,
and for local network access the first time it reaches a camera. Allow both.

### From source, on a Mac or Linux

```sh
make run-server     # terminal 1: data in ~/PTZProgrammer, listens on http://127.0.0.1:8787
make run-app        # terminal 2
```

### Check the install

With the server running on the simulator:

```sh
ptz-probe
```

It takes the edit lease, makes a scratch song, tries every transport action in
every state (play, arm, E-Stop and the rest), and removes the scratch song. It
should end without a failure. It refuses to run against a real rig.

---

## 3. Start the server and connect

### Start the server

```sh
ptz-server
```

| Option | Use |
| --- | --- |
| `--data <dir>` | Where the show is kept. `~/PTZProgrammer` unless given. |
| `--bind <addr:port>` | Where it listens. `127.0.0.1:8787` unless given. |
| `--access-key <key>` | Required on every request. The server refuses to listen beyond this machine without one. |
| `--no-seed` | Do not create the example songs and presets in an empty data folder. |
| `--audio-device <name>` | Play through the output whose name contains this text. |
| `--no-audio` | Never open an audio device; run silent. |
| `--list-audio` | List the audio devices and exit. |
| `--selftest-audio` | One second of tone on the left, then the right, and exit. |
| `--verify-log` | Check the run log and exit. |

The server keeps running when the app is closed, and the show keeps playing.
To stop it, press Ctrl-C in its terminal.

A new data folder is seeded with three presets (Stage Wide, Lectern MCU, Awards
Two-Shot) and three songs (Walk-In, Opening Number, Awards Bed) so there is
something to look at. Use `--no-seed` for a clean start.

### Open the app

Open **PTZ Programmer**. It tries `http://127.0.0.1:8787` straight away. If no
server answers, the connect screen appears:

| Field | What to enter |
| --- | --- |
| Show server | The server's address, for example `http://127.0.0.1:8787` |
| Station name | A name for this station. Other stations see it when you hold the edit lease. |
| Access key | Only for a server on another machine |

**Start a server on this computer** runs `ptz-server` with its defaults (the
simulator rig, data in `~/PTZProgrammer`) and connects to it. That server keeps
running after the app quits.

### Take the edit lease

One station at a time may change the show. The right-hand end of the header
says who that is. Press **Take the lease** to make it you; the header then
reads *You hold the edit lease*. Everything in the rest of this guide needs it.

---

## 4. The project: frame rate and cameras

Project settings live in the **Inspector** tab when nothing on the timeline is
selected (click an empty part of the timeline first).

### Frame rate

Under **Project**, choose the **Frame rate**: 23.976, 24, 25, 29.97 drop-frame
or 30. Set it once, before programming; it must match the timecode the show
will be locked to. Code arriving at another rate is refused.

### Cameras

Under **Cameras**:

1. The project starts with one camera. Type a name for it in its name field
   (*Centre back*, *Left wall*: names you will recognise on the camera bar).
2. **Add a camera** makes another, up to twelve. A new camera copies the
   selected camera's lens, limits and rules, starts on the simulator, and
   stands two metres to the right of the one it was copied from.
3. **Move up** and **Move down** change the numbering. The number is the key
   that selects the camera (`1` to `9`, then `0`, `-`, `=` for ten to twelve).
4. **Remove camera** takes the selected camera out. Its presets go with it;
   any moves it has on a timeline stay as issues that block Arm until deleted.

Adding and removing cameras needs the show stopped and disarmed.

**When a camera faults** chooses what the other cameras do during a live show:
*Hold that camera, the others carry on* (the default), or hold them all.
E-Stop always stops every camera whatever this says.

Everything below this point in the Inspector (lens, position, rig, limits,
keep-out rules) belongs to **the selected camera**. Select each camera in turn
from the camera bar and set it up.

### The lens on each camera

Under **Lens**:

| Setting | What to enter |
| --- | --- |
| Lens | Pick the FR7 kit zoom (FE PZ 28-135mm F4 G OSS) or one of the other listed zooms, or type your own |
| Focal length | The widest and the longest end, in millimetres. A prime has the same number twice. |
| Aperture | The widest aperture |
| Sensor | Full frame, Super 35, or a typed width: the width of the part of the sensor that is recorded, 16:9 |

The Inspector then shows, for the zoom the camera is at now: the focal length,
the angle of view, where the centre of the frame lands on the stage model, how
big the frame is there, and the range that is sharp. Tick **At a distance of**
to get those numbers for a distance you type instead.

Every picture in the app is drawn from these numbers, so a wrong lens gives
wrong framing everywhere. Zoom is taken to run evenly in stops from the wide
end to the long end; bring-up (Part 7) can check that against the real lens.

---

## 5. The venue: stage model and camera positions

Until you import a model the monitors show a small built-in stage. Importing
the venue makes the Program monitor show what each camera will actually frame
from where it stands.

### Import the model

- Drop an `.obj` or `.3ds` file on the app window, or
- in the Inspector under **Stage**, press **Import a model**, or type the
  file's path in the field beside it and press Enter.

An OBJ's `.mtl` file is read from the same folder for its colours. Textures are
not used; each surface takes its material's colour. The limit is 1.5 million
triangles and 200 MB. The app sends the server a compact copy, so every station
sees the same model.

### Make it the right size and the right way up

The Inspector shows the model's size in metres. Check it against a dimension
you know (the stage width, say).

| If | Change |
| --- | --- |
| The size is wrong by a factor (1000, 100, 39.37, 3.28) | **One unit is**: millimetres, centimetres, metres, inches or feet |
| The model lies on its side | **Up**. OBJ files are taken as Y up and 3DS files as Z up to start with. |
| It faces the wrong way | **Turned**: degrees about the vertical, counter-clockwise seen from above |
| It is off-centre or floating | **Moved**, or **Sit on the floor** (lowest point to height zero) and **Centre** (middle of its footprint over the origin) |

The convention throughout: distances are metres, Z is up, and a camera with
heading 0 and pan 0 looks along +Y, towards the stage.

**Remove the model** brings the built-in stage back for every station.

### Place the cameras

Open the **Stage** tab. It shows the model from outside with every camera, its
name, and the cone it sees.

| Tool | What it does |
| --- | --- |
| **Select** | Click a camera or its name to select it. Drag empty space to look around. |
| **Move** | Drag a camera across the floor. With **Snap to model** it follows the surface under the pointer, so it can stand on a riser or a balcony. |
| **Rotate** | Drag around a camera to turn where its pan 0 points. Shift steps by 5 degrees. |
| **Place** | Click where the selected camera should stand. |
| **Aim** | Click what the selected camera's pan 0 should face. |
| **Measure** | Click two points to read the distance between them. |

**Perspective**, **Top**, **Front** and **Side** choose the view and fit the
stage to it. In the flat views **Cut** hides a ceiling or a near wall.
Right-drag or Shift-drag slides the view; the scroll wheel zooms. Escape
cancels a drag and **Undo move** puts the last camera back.

A practical order for each camera:

1. Switch to **Top**, choose **Place**, and click where the pedestal stands.
2. Choose **Aim** and click the centre of the stage, or use **Face the stage**
   in the Inspector.
3. In the Inspector under **Position on the stage**, type the exact **Floor
   position** if you have measured it, and set **Lens height**: the height of
   the lens above the floor with the column fully down.
4. If the pedestal stands on a riser or balcony, press **Drop to the surface
   below**.
5. Open the **Program monitor** and check that the picture looks like the view
   from that seat. Use **Measure** against a real tape-measure distance if it
   does not.

The **Multiview** tab shows all the cameras' pictures at once: a quick check
that nobody is pointing at a wall. Placing cameras is refused while armed.

---

## 6. Sound and timecode

All of these are devices **on the server machine**, set in the Inspector with
nothing selected.

### Audio output

```sh
ptz-server --list-audio            # see the device names
ptz-server --audio-device "USB"    # start with the output whose name contains this
ptz-server --selftest-audio        # tone on the left, then the right
```

Under **Audio output**, route each channel:

| Choice | What comes out |
| --- | --- |
| Program (the song's audio) | The mono mix of every unmuted audio clip |
| LTC timecode | The song's timecode, generated while it plays. **LTC level** sets how loud. |
| Silent | Nothing |

The default is Program on the left and Silent on the right. The transport bar
shows a meter for each channel and names the device. If no device can be
opened the server runs silent on the system clock, and the bar says why.

While a song plays free-run, the playhead is driven by the sound card's own
count of samples played, so picture and sound cannot drift apart.

### Timecode inputs

Under **Timecode inputs**, set where **Lock** will listen:

| Input | Setting |
| --- | --- |
| LTC input | The audio input device and the channel the LTC is on |
| MTC input | The MIDI port |
| Art-Net | The address to listen on. `0.0.0.0:6454` hears every interface. |
| Freewheel | How many frames the playhead carries on when code drops out before it stops and holds. Eight to start with. |

**List devices again** rescans after you plug something in.

### Prove it

1. Open the seeded song **Walk-In** and press **Space**. You should hear one
   click 2.5 seconds in, on the channel routed to Program, and see its meter
   move.
2. If you routed LTC out, feed it to something that reads timecode and check
   that it shows the song's timecode.
3. If the show will follow incoming code: press **Lock**, choose the input,
   start the source. The bar reads WAITING FOR, then LOCKED TO, and the
   playhead follows. The tooltip on the chip shows the last code received.

---

## 7. The real rig

Read this part through once before you connect anything.

> **What has and has not been tested.** The FR7 adapter was written from Sony's
> published VISCA and CGI documents. The AutoPod+ adapter follows an exchange
> recorded between a real AutoPod+ hand controller and leg box. Both have been
> tested only against software stand-ins built from the same reading. Neither
> has yet driven a real camera or pedestal from this program. Bring-up, below,
> is the step that finds out on your hardware, with someone watching, before a
> show does. Do not skip it.

Do the whole of this part for one camera, then the next.

### 7.1 Try it against the stand-ins first

```sh
ptz-bringup fakes               # one FR7 and one AutoPod+ on this machine
ptz-bringup fakes --count 12    # twelve pairs
```

The stand-in FR7 listens on `127.0.0.1:52381` (web interface on 8088, login
`admin` / `fr7-password`) and the stand-in AutoPod+ on `127.0.0.1:8089`. With
`--count`, each further rig takes the next ports up. Point a camera's rig
settings at them (7.3) and go through arming and playing live. Nothing real
moves, and you learn the procedure.

### 7.2 Measure the rig: bring-up

Stop the show server first, or set that camera's rig back to the simulator, so
that only one program is talking to the devices. Clear the area around the
camera: this tool moves it. Keep a hand on the camera's own stop.

```sh
ptz-bringup --fr7 192.168.0.100 --autopod 192.168.0.110 --user admin --password "..."
```

The password can come from the `PTZ_FR7_PASSWORD` environment variable
instead. The tool runs seven steps. Each says what it will move and asks before
it does, and brings the rig back to where it stood.

| Step | What it settles |
| --- | --- |
| `connect` | Both devices answer; the web login works; the AutoPod+ names its model and firmware, is switched on and is calibrated |
| `moves` | Which way the counts run on each axis, counts per degree, and that zoom and focus reach a position |
| `timing` | How long a position query takes, how fast commands can be sent, the query delay |
| `speeds` | The real speed of each of the 24 VISCA speed steps |
| `tracking` | Whether steering the head along a curve or sending plain moves follows better, and a tolerance |
| `autopod` | The height map (you measure the column at three heights), whether speed is linear, top speed |
| `estop` | How long each device takes to stand still after a stop, and how far it travels |

Have a tape measure ready for the `autopod` step. Useful options:

| Option | Use |
| --- | --- |
| `--only tracking,estop --from report.json` | Repeat chosen steps, starting from an earlier report |
| `--quick` | Shorter runs: five speed steps instead of all 24 |
| `--report <file>` | Where to write the report. `bringup-<date>.json` unless given. |
| `--fr7-http <port>` | The FR7's web port if it is not 80 |
| `--yes` | Do not ask before moving. The height map is skipped, because it needs someone to measure. |

The report is printed in words and written as JSON. Read it. If a step failed,
fix the cause and repeat that step before going on.

### 7.3 Put the measurements into the project

Start the show server again, then:

```sh
ptz-bringup --apply bringup-2026-11-14.json --camera 3
```

`--camera` takes the camera's number in the camera list, or its id; without it
the values go to camera 1. Add `--server` and `--access-key` for a server
elsewhere. The tool takes the edit lease, writes the values into that one
camera and releases the lease. Top speeds are only ever lowered by a report,
and which rig the server drives is never changed by it.

### 7.4 Point the camera at its rig

Select the camera, and in the Inspector under **Rig** set **Drives** to **FR7
and AutoPod+**. This can only be changed while stopped and disarmed. A show can
mix real rigs and simulated ones.

| Setting | What it is |
| --- | --- |
| FR7 address | The camera's IP address. VISCA over IP is UDP port 52381 unless you add a port. |
| FR7 web port, FR7 login | The camera's web interface, for preset names and pictures |
| AutoPod+ address | The leg box. Port 80 unless you add one. |
| Counts per degree, Counts run backwards | How the head reports pan and tilt. From bring-up. |
| Speed steps | The speed of each VISCA step. Shows *assumed* until bring-up has measured them. |
| Plain moves | Send a preset move as one command and let the head run it. Use it if bring-up showed that steering strays. |
| Query delay | How old a position is when it arrives. Zero uses the round trip measured as it runs. |
| Arm moves at | The share of top speed used to travel to the playhead when arming. Half to start with. |
| Tracking fault after, Tolerance | How far and for how long an axis may be off the timeline while live before that camera is held |
| AutoPod+ height map | Raw leg-box position against measured height, at three heights or more. **Add the current height** takes the raw value from the leg box. |

The camera bar shows the camera's state, and hovering its chip shows each
device, for example *AutoPod+ V3.49e at 192.168.60.204:80 answers in 3 ms*. An
AutoPod+ in standby blocks Arm; the program never switches it on by itself.

### 7.5 Set the limits

Under **Rig limits**, for each axis (pan, tilt, zoom, focus, elevation) set the
**Min** and **Max** of the range this camera may use where it stands, its **Top
speed** and its **Accel**. Set them to what this rig does safely in this
position, not to what the hardware can do: a camera beside a wall should not be
able to pan into it. Auto time and the too-fast warning are worked out from
these.

Under **Keep-out rules**, add a rule wherever two axes together can do damage
that neither does alone. **Add a keep-out rule** starts with the usual one:
*when elevation is below 200 mm, tilt must be above -10 degrees*, which stops
the camera looking down into its own pedestal. Edit it to fit.

Limits and rules are enforced twice: on the timeline, where a move that breaks
one is an issue that blocks Arm and cannot be accepted, and live, where the
pose about to be sent is checked every cycle and the camera is held instead.

### 7.6 Read the camera's presets

In the **Presets** tab, **Sync from rig** reads the selected camera's presets.
On a real rig the camera is recalled to each preset in turn so that its pose
and picture can be read, so it moves, at its own speed. The app asks first. The
pedestal does not move. Sync is refused while armed or playing, and E-Stop
abandons it.

### 7.7 First live move

With the area clear and one hand on E-Stop:

1. Make a short song with one slow move for this camera. Save and Commit.
2. Press **Arm (moves rig to playhead)**. The rig travels to the pose at the
   playhead. The bar reads ARMING, then ARMED.
3. Press **Play Live**. Watch the camera, and the **Rig controller** tab, which
   shows how far each axis is from the timeline against its tolerance.
4. Press **Stop**, then **E-STOP**. Check that everything stands still, then
   **Reset E-Stop** and **Disarm**.

If the camera strays, is held with a tracking fault, or moves the wrong way,
stop and go back to bring-up for the step that covers it.

---

## 8. Operator stations

To run the app on another machine, start the server so that it listens on the
network, with a key:

```sh
ptz-server --bind 0.0.0.0:8787 --access-key "choose-a-key"
```

On the other machine, enter the server's address (`http://192.168.0.10:8787`)
and the key on the connect screen. The address, station name and key are
remembered.

- One station holds the **edit lease** and can change the show. The others
  watch: they see the timeline, the monitors and the transport follow along.
- A watching station can **Take over**. The header of the station that held
  the lease then reads *Watching* and names the new holder.
- A station that goes silent for ten seconds loses the lease by itself.
- **E-STOP works from every station**, with or without the lease.
- If a station loses the server, a red banner says so. The show keeps running
  on the server; that window is simply not in control until it reconnects.

The sound comes out of the server machine, not the stations.

---

## 9. Before the first show

Work down this list with the rig powered and the area clear.

**Project**

- [ ] Frame rate matches the show's timecode.
- [ ] Every camera is named, numbered in a sensible order, and has the right lens.
- [ ] The stage model is the right size, and every camera's Program picture
      matches its real view.

**Each real camera**

- [ ] Bring-up has run all seven steps on this rig and its report is applied.
- [ ] **Drives** is *FR7 and AutoPod+*, and both devices show as answering.
- [ ] Rig limits are set for where this camera stands. Keep-out rules are in.
- [ ] The AutoPod+ is switched on, calibrated, and its height map has three
      heights or more.
- [ ] A slow test move was armed, played live and E-Stopped, and the camera
      stood still.

**Sound and timecode**

- [ ] The click in Walk-In is heard on the right output.
- [ ] If used, LTC out is read correctly downstream.
- [ ] If the show is locked: the source is chosen, code is received at the
      project's rate, and the playhead follows it.

**Stations and safety**

- [ ] Every station can reach the server, and E-STOP has been pressed once
      from each one.
- [ ] Everyone near the rig knows where E-STOP is and that Arm moves the cameras.
- [ ] `ptz-server --verify-log` reports the run log intact.
- [ ] The data folder has been backed up (Part 10).

---

## 10. Looking after it

### Where things are kept

The server's data folder (`~/PTZProgrammer` unless `--data` says otherwise):

| Path | What it is |
| --- | --- |
| `project.json` | Frame rate, cameras, lenses, positions, rig settings, limits, rules, audio and timecode settings |
| `presets.json` | Every camera's presets |
| `songs/<id>/` | Each song: the working copy, the last Save, every committed revision (`rev/`), and recorded passes (`passes/`) |
| `media/<id>/` | Each imported file, its decoded audio, its waveform, and a video's frame pictures |
| `stage/model.ptzmesh` | The imported stage model |
| `log/<date>.jsonl` | The run log |

Every file is written to a temporary name and then renamed, so a crash or a
power cut never leaves a half-written song.

The app keeps only its own window settings (server address, station name,
access key, panel layout): on a Mac in
`~/Library/Application Support/PTZ Programmer/settings.json`, on Linux in
`~/.config/PTZ Programmer/settings.json`.

### Back up and move a show

Stop the server and copy the whole data folder. To restore it, or to move the
show to another machine, copy it back and start the server with `--data`
pointing at it.

### The run log

Everything the server does to the show is written to the run log and shown in
the **Log** tab: each transport action and who asked for it, each refusal and
its reason, what was armed (song, revision and its SHA-256), the rig's pose and
error once a second while live, faults, how long an E-Stop took, devices
connecting and dropping. Each record carries the hash of the one before it:

```sh
ptz-server --verify-log
```

walks the chain and names the first record that was changed or removed.

### Update

Pull the new source and run `make install-mac` again. It replaces the app and
the tools. The data folder is not touched, and a project saved by an earlier
version is brought up to date when the server opens it. Back up first.

### Remove

Delete `~/Applications/PTZ Programmer.app` and the three links in
`~/.local/bin`. The data folder and the app's settings file stay until you
delete them.

---

## 11. When something is wrong

| What you see | What it means | What to do |
| --- | --- | --- |
| The connect screen: *Could not reach ...* | No server is answering at that address | Start the server, or check the address and port. For another machine, check that the server was started with `--bind` and that the access key matches. |
| The server will not start with `--bind 0.0.0.0:8787` | It refuses to listen beyond this machine without a key | Add `--access-key` |
| A red banner: the server is not reachable | This station lost the server | The show keeps running on the server. Check the network; the app reconnects by itself. |
| *Watching. ... holds the edit lease* | Another station is editing | **Take over** if it should be you |
| A button is disabled | The server will not allow that action now | Hover it. It says why. |
| *No audio output*, or the bar says the device could not be opened | The server found no output | `ptz-server --list-audio`, then start with `--audio-device`. Until then the show runs silent on the system clock. |
| No sound, but the meters move | The output is routed to another channel or device | Check **Audio output** in the Inspector and the device named on the bar |
| A video import is refused | `ffmpeg` and `ffprobe` are not on the server machine | `brew install ffmpeg` on the server machine |
| The stage model is enormous or tiny | Wrong unit | Change **One unit is** |
| The stage model lies on its side | Wrong up axis | Change **Up** |
| Lock: the bar names a different rate | Code is arriving at a rate that is not the project's | Fix the source, or the project's frame rate |
| Lock stays on WAITING FOR | No code is arriving, or it is outside the song | Check the input device and channel under **Timecode inputs**; check the song's start timecode |
| A camera chip says *no answer* | A device is not answering | Hover the chip to see which. Check power, cabling and the address under **Rig**. |
| Arm is refused: *AutoPod+ is not calibrated* | The leg box reports that it has not been calibrated | Calibrate it at the AutoPod+ |
| Arm is refused: *The AutoPod+ is in standby* | The leg box is switched to standby. The program never switches it on by itself. | Switch it on at the leg box |
| Arm is refused: *Commit the song first* | Only a committed revision with no unsaved edits can be armed | Save, then Commit |
| Arm is refused: *Fix or accept N timeline issues first* | Moves break a limit | Click the issue count in the header to go to the first one |
| A camera is held, with a reason in red | It strayed from the timeline, would have crossed a limit, or stopped answering | The others carry on. Disarm and Arm again to bring it back, after finding out why. |
| *E-STOP LATCHED* | E-Stop was pressed | Make sure it is safe, then **Reset E-Stop** |
