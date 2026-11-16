# Motion Choreographer: Setup Guide

This guide takes a new installation from nothing to a system that is ready to
program: software installed, the show server running, the project describing
your cameras, lenses and venue, sound and timecode connected, and each real rig
measured and proven safe. Programming in the app is the
[Programming Manual](programming-manual.md). Programming from a SKAARHOJ PTZ Pro,
including how to connect it, is [PTZ Pro: setup and programming](ptz-pro.md).

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
| [8. Operator stations](#8-operator-stations) | A second machine, the edit lease, a hardware panel |
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
- **Optionally, a SKAARHOJ PTZ Pro panel**, to run the show from hardware
  controls. See [A SKAARHOJ panel as a station](#a-skaarhoj-panel-as-a-station).

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
| SKAARHOJ panel, if used | server (or the machine running `ptz-panel`) | the panel | TCP 9923 |

### Sound and timecode (as your show needs)

- An audio output on the server machine for the song's audio. A stereo
  interface gives you the mix on one channel and LTC timecode on the other.
- An audio input on the server machine if the show follows LTC.
- A MIDI interface on the server machine if the show follows MTC.
- `ffmpeg` and `ffprobe` on the server machine if you want reference video
  (`brew install ffmpeg`). Audio does not need them.

### To build the software

- Rust 1.95 or newer (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`).
- On a Mac, the Xcode command line tools (`xcode-select --install`).
- On Linux, the ALSA headers (`libasound2-dev`) and the usual X11 or Wayland
  libraries.

---

## 2. Install

There are two ways to install: build **installers** once and run them on each
machine, or build and install straight onto the Mac you are sitting at.

### With the installers

On a machine with the build tools above, from the project folder:

```sh
make package
```

That writes two installers into `dist/`, for the kind of machine it was run on:

| Installer | On a Mac | On Linux | Use it on |
| --- | --- | --- | --- |
| The app, with the show server as an option | `Motion-Choreographer-<version>-macos-<arch>.pkg` | `motion-choreographer-<version>-linux-<arch>.run` | Every station with a screen |
| The show server alone | `Motion-Choreographer-Server-<version>-macos-<arch>.pkg` | `motion-choreographer-server-<version>-linux-<arch>.run` | A machine that only runs the show |

A Mac installer has to be built on a Mac, because macOS programs cannot be
compiled anywhere else. It is built once; the machines you install on need
none of the build tools. On the Mac that builds:

1. Install the Xcode command line tools (`xcode-select --install`) and Rust
   (see [To build the software](#to-build-the-software)).
2. In the project folder, run `make package`. The first build takes several
   minutes. The script checks both tools first and says what to run if one is
   missing or too old, and it opens each installer again when it has made it,
   to check that the parts are inside.
3. The two `.pkg` files are in `dist/`. Copy them to the other Macs.

`make package` builds for the kind of Mac it runs on. An Apple Silicon Mac
also builds the pair for Intel Macs:

```sh
PTZ_TARGET=x86_64-apple-darwin make package
```

**Which parts go where.** The show server runs on one machine. Every station
needs the app. So:

- one computer doing everything: the app installer, with the server;
- the show machine plus remote stations: the app installer with the server on
  the show machine (or the server installer, if it has no screen), and the app
  installer *without* the server on each remote station.

**On a Mac**, double-click the `.pkg`. The **Installation Type** step lists
two parts: *Motion Choreographer*, which is always installed, and *Show server and
tools*, which is ticked to start with. Clear it for a station that only
connects to a server elsewhere.

- The installers are not signed with an Apple Developer ID, so the first time
  macOS will not open one on a double-click. Right-click it and choose
  **Open**, or run `sudo installer -pkg <file> -target /`.
- The app goes to `/Applications/Motion Choreographer.app`. A copy under the
  app's old name, `PTZ Programmer.app`, is removed.
- The server and tools (`ptz-server`, `ptz-probe`, `ptz-bringup`, `ptz-panel`) go to
  `/usr/local/ptz-programmer/bin`. New terminal windows find them by name.

**On Linux**, run the installer:

```sh
sh motion-choreographer-<version>-linux-<arch>.run   # asks whether to install the server too
sh motion-choreographer-<version>-linux-<arch>.run --no-server
sh motion-choreographer-server-<version>-linux-<arch>.run
```

It installs under `~/.local` (`--prefix` chooses somewhere else), adds PTZ
Programmer to the applications menu, and says so if a library the programs
need is missing. `--help` lists the options.

An app installed without the server greys out **Start a server on this
computer** on the connect screen. Enter the address of the show machine
instead.

### Straight onto this Mac

From the project folder:

```sh
make install-mac
```

It builds for the Mac it runs on, Apple Silicon or Intel, and stops with the
command to run if Rust is missing or older than 1.95. The first build takes
several minutes. When it finishes you have:

- `~/Applications/Motion Choreographer.app`, the desktop app, with the command-line
  tools inside it;
- `ptz-server`, `ptz-probe`, `ptz-bringup` and `ptz-panel` linked into `~/.local/bin`.

If the installer says `~/.local/bin` is not on your PATH, run the line it
prints, then open a new terminal.

The app is signed ad hoc, which is enough to run it on the Mac that built it.
A second Mac (a remote station, say) builds its own copy the same way; a copy
carried across is refused by macOS. macOS may ask for microphone access the
first time the server listens for LTC, and for local network access the first
time it reaches a camera. Allow both.

**To install a newer version**, quit the app, stop the server (Ctrl-C in its
terminal), and run `make install-mac` again. The installer refuses to replace
the app while it is running. The show data is not touched. A copy from
before the app was renamed, `~/Applications/PTZ Programmer.app`, is removed
when the new one goes in.

**To remove it**, run `make uninstall-mac`. That removes the app and the three
links. It leaves the show data (`~/PTZProgrammer`) and the app's settings
file (see [Where things are kept](#where-things-are-kept)); delete those by
hand if you want them gone.

This way of installing keeps everything in your own home folder, with the
server inside the app. Do not mix it with the installers on one Mac: remove
one before using the other.

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

It makes a scratch song, tries every transport action in every state it can
reach without timecode (play, arm, E-Stop and the rest), and removes the
scratch song. It should end without a failure. It refuses to run against a
real rig, or while a show is armed or playing.

Run it before real work starts, not during: it takes the edit lease from
whoever holds it, presses E-Stop, and slows the Arm speed while it runs. If it
is interrupted, a song called *Probe Scratch* and the slowed Arm speed
(Show settings, **Rig connection**) are left behind to put right by hand.

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
| `--panel <address>` | Run the show from a SKAARHOJ panel at this address as well. See [Part 8](#a-skaarhoj-panel-as-a-station). |
| `--panel-listen <addr:port>` | The same, for an older panel that dials in: wait for it here. |

The server keeps running when the app is closed, and the show keeps playing.
To stop it, press Ctrl-C in its terminal. **Disarm first.** Stop the server
only when the show is disarmed: see [Safety](#safety-read-this-before-arming).

A new data folder is seeded with three presets (Stage Wide, Lectern MCU, Awards
Two-Shot) and three songs (Walk-In, Opening Number, Awards Bed) so there is
something to look at. Use `--no-seed` for a clean start.

### Open the app

Open **Motion Choreographer**. It tries the server it last used straight away
(`http://127.0.0.1:8787` the first time). If no server answers, the connect
screen appears:

| Field | What to enter |
| --- | --- |
| Show server | The server's address, for example `http://127.0.0.1:8787` |
| Station name | A name for this station. Other stations see it when you hold the edit lease. |
| Access key | Only for a server on another machine |

Servers this station has connected to before are listed under the address;
click one to use it.

**Start a server on this computer** runs `ptz-server` with its defaults (data
in `~/PTZProgrammer`) and connects to it. A new data folder starts on the
simulator; an existing one drives whatever rig its project names, real cameras
included. That server keeps running after the app quits: stop it from Activity
Monitor, or with `pkill ptz-server`.

### Take the edit lease

One station at a time may change the show. The right-hand end of the header
says who that is. Press **Take the lease** to make it you; the header then
reads *You hold the edit lease*. Everything in the rest of this guide needs it.

---

## 4. The project: frame rate and cameras

Everything that belongs to the whole show lives in **Show settings**. Press
**Show settings** at the right end of the camera bar (the Inspector, with
nothing selected, has a button that goes there too). It takes the place of the
timeline and monitors; the header, the transport with E-STOP and the camera
bar stay on screen. **Close settings**, in the same place, or Esc brings the
timeline back.

The pages are listed down the left side, in three groups:

| Group | Pages |
| --- | --- |
| The show | Cameras, Frame rate and faults, Stage model, Programming |
| The selected camera | Rig connection, Lens, Position on the stage, Rig limits, Keep-out rules |
| The server | Audio output, Timecode inputs, Control panel |

Settings can be changed while you hold the edit lease and the show is
disarmed. Otherwise the page says why, and they can only be read.

### Frame rate

On the **Frame rate and faults** page, choose the **Frame rate**: 23.976, 24, 25, 29.97,
29.97 drop-frame (shown as 29.97df) or 30. Set it once, before programming; it
must match the timecode the show will be locked to. Code arriving at another
rate is refused.

29.97 is non-drop: thirty numbers every second and none skipped, written with
colons, so an hour of code lasts 3.6 seconds longer than an hour on the clock.
Timecode itself cannot tell 29.97 non-drop from 30, or 23.976 from 24: the
project's rate decides. Choose the one the source really runs at, or audio and
moves drift from the code by a frame every 33 seconds.

### Cameras

On the **Cameras** page:

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

**When a camera faults**, on the **Frame rate and faults** page, chooses what
the other cameras do during a live show:
*Hold that camera, the others carry on* (the default), or hold them all.
E-Stop always stops every camera whatever this says.

The five pages under **The selected camera** (rig connection, lens, position,
limits, keep-out rules) belong to one camera, named at the top of the page.
Select each camera in turn from the camera bar, or with its number key, and
set it up; the page stays open.

### The lens on each camera

On the **Lens** page:

| Setting | What to enter |
| --- | --- |
| Lens | Pick the FR7 kit zoom (FE PZ 28-135mm F4 G OSS) or one of the other listed zooms, or type your own |
| Focal length | The widest and the longest end, in millimetres. A prime has the same number twice. |
| Aperture | The widest aperture |
| Sensor | Full frame, Super 35, or a typed width: the width of the part of the sensor that is recorded, 16:9 |

The page then shows, for the zoom the camera is at now: the focal length,
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
- in Show settings on the **Stage model** page, press **Import a model**, or type the
  file's path in the field beside it and press Enter.

An OBJ's `.mtl` file is read from the same folder for its colours. Textures are
not used; each surface takes its material's colour. The limit is 1.5 million
triangles and 200 MB. The app sends the server a compact copy, so every station
sees the same model.

### Make it the right size and the right way up

The Stage model page shows the model's size in metres. Check it against a dimension
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

The pad at the top right of the picture does the same with plain clicks, for a
mouse with no wheel: **+** and **-** zoom, the arrows turn the view or slide
it, and the middle button shows everything. The two buttons at its top corners
choose whether the arrows, and a plain drag on the picture, turn the view or
slide it. Hold a button to keep going.

A practical order for each camera:

1. Switch to **Top**, choose **Place**, and click where the pedestal stands.
2. Choose **Aim** and click the centre of the stage, or use **Face the stage**
   in Show settings under **Position on the stage**.
3. On that same page, type the exact **Floor
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

All of these are devices **on the server machine**, set in Show settings on
the **Audio output** and **Timecode inputs** pages.

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

The default is Program on the left and Silent on the right. While a song plays
a file whose LTC channel is read, Program goes to both outputs, and while the
playhead is locked to timecode from outside, the song's files are not played at
all: the sound comes from where the timecode does. The transport bar
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

### Safety: read this before arming

- **E-STOP on the screen is a software stop.** It is a command the server
  sends over the network. It does not replace the AutoPod+'s own stop control
  or cutting the power, and it cannot reach the rig if the server or the
  network is down. Before the first live move, find the pedestal's own stop
  and the power, and keep someone within reach of them.
- **Disarm before you stop the server.** Stopped with Ctrl-C (or a normal
  shutdown of the machine), the server stops every camera and pedestal on its
  way out. If it is killed outright, loses power or crashes, it cannot: a rig
  that was moving keeps its last speed command until the camera's or
  pedestal's own limits end the move. Treat a dead server while live as a
  reason to use the hardware stop. Starting the server again also stops it:
  the server sends each camera and pedestal a stop as soon as it connects.
- **One controller at a time.** Another controller sending to the same FR7 (a
  remote, a second copy of this program, a vision mixer's PTZ control) fights
  this one. Take the others off the camera while a show is armed.
- **Arm, Cue and a click on the ruler move the rig.** While armed and stopped,
  moving the playhead (or a jump in the timecode the show is locked to) sends
  every camera to the pose there.
- **Going live switches the FR7's focus to manual**, so the timeline can drive
  it. Nothing switches it back afterwards; set it at the camera if you want
  autofocus again.
- **A camera with no AutoPod+ address runs without a pedestal.** If **Drives**
  is FR7 and AutoPod+ and the AutoPod+ address is empty, only the head is
  driven and the elevation is taken to be whatever the timeline says.

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

Select the camera, and in Show settings under **Rig connection** set **Drives** to **FR7
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

An FR7 keeps speed settings of its own: a pan and tilt speed scale, a zoom speed
scale, a slow mode, and whether it counts 24 speed steps or 50. Every speed
asked of the camera goes through them, so one left turned down runs slower than
the timeline expects, and one counting 50 steps tops out near a fifth of its
speed. Each time the show server connects to an FR7 it sets both scales to 100,
slow mode off and 24 speed steps, and the camera's chip says what it changed,
for example *Raised to full speed: pan and tilt speed scale from 58 to 100*.
Bring-up does the same before it measures. The camera keeps the new values after
the server stops, so any other controller that drives the same camera has to be
set for 24 speed steps too.

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
- A watching station can **Take over**. It is asked to confirm, because the
  other station loses control at once. The station that held the lease is told
  who took it, and its header then reads *Watching* and names the new holder.
- A station that goes silent for ten seconds loses the lease by itself.
- **E-STOP works from every station**, with or without the lease.
- If a station loses the server, a red banner says so. The show keeps running
  on the server; that window is simply not in control until it reconnects.

The sound comes out of the server machine, not the stations.

### A SKAARHOJ panel as a station

A SKAARHOJ PTZ Pro can be an operator station. It talks SKAARHOJ's Raw Panel
protocol to a small program, `ptz-panel`, which holds the edit lease under the
name **Panel** and asks the server for the same things the app does. Connecting
it and programming a song from it is
[PTZ Pro: setup and programming](ptz-pro.md). Every key is also listed in the
[Programming Manual](programming-manual.md#14-programming-from-a-panel).

> **Not yet run on a real panel.** This was built and tested against a
> stand-in panel that follows SKAARHOJ's published Raw Panel protocol and the
> published layout of the PTZ Pro. No real PTZ Pro has been connected to it.
> Work through [First time on a real panel](#first-time-on-a-real-panel) with
> every rig disarmed before you rely on it.

**The base setup stays in the app.** Cameras, lenses, the stage model, rig
addresses and limits, audio and timecode inputs, and importing media are done
in the app as in Parts 4 to 7. Everything after that can be done on the panel.

**Network.** Put the panel on the show's wired network with a fixed address.
The program connects to the panel on TCP port 9923. Nothing else should be
driving the panel at the same time.

**On the panel.** The PTZ Pro has to be in Raw Panel mode. SKAARHOJ's
documentation describes it as follows; it has not been checked here on a
panel:

1. Open the panel's own web page and its system manager (version 1.0.6 or
   later).
2. On the Settings page, switch on **Raw Panel Mode**. The panel offers to
   switch off its own Reactor program: accept. Only one of the two can drive
   the panel.
3. Leave the port at 9923, or note what you change it to.

**Start it.** The usual way is in the app: open **Show settings**, choose
**Control panel**, type the panel's address and tick **Use this panel**. You
need the edit lease, and the show must not be armed. The address is saved with
the show, so the server connects to the panel every time it starts, and the
page says whether the panel is connected, still being looked for, or what is
in the way.

You can also give the address where the server is started. That panel is then
used whatever Show settings says, and is changed there, not in the app:

```sh
ptz-server --panel 192.168.0.60
```

Or run `ptz-panel` yourself, on the server machine or on any machine that can
reach both the server and the panel:

```sh
ptz-panel --panel 192.168.0.60
ptz-panel --panel 192.168.0.60 --server http://192.168.0.10:8787 --access-key "choose-a-key"
```

| Option | Use |
| --- | --- |
| `--panel <address>` | The panel. Port 9923 unless you add `:port`. |
| `--listen <addr:port>` | Instead of `--panel`: wait here for an older panel that is set to dial in. With the server: `--panel-listen`. |
| `--server <url>` | The show server. `http://127.0.0.1:8787` unless given. |
| `--access-key <key>` | The server's key, or set `PTZ_ACCESS_KEY`. |
| `--station <name>` | The name the panel holds the lease under. *Panel* unless given. |
| `--map <file>` | The control map. `panel.json` in the folder you start it in, if there is one. |
| `--lease-grace <seconds>` | How long a lost panel keeps the lease. 30 unless given. |

It keeps trying for ever: a panel or a server that is not there yet is waited
for, and one that drops is reconnected to. The terminal says when the panel
and the server connect and when either is lost. When the panel is connected
its CAM 9 display reads **E-STOP / Press** in red and the top left of the big
display shows the song and the timecode. The **Control panel** page says whether it is connected, still being looked
for, or what is in the way. Once the panel takes the lease the app names
*Panel* as the holder.

Stopping `ptz-panel` with Ctrl-C does not stop the show. Hand control stops,
the lease is given back, and the panel's keys go dark with **Panel / Controller
stopped** on the big display, so that the panel is not left showing a show
nobody is behind. If the program is killed outright instead, the server stops
the hand-driven camera a quarter of a second later, the lease frees itself
after ten seconds, and the panel keeps its last picture.

A server started with `--panel` that is told to stop sends its stop to every
rig first, as it always does, and tells the panel afterwards. The panel is
never waited for longer than a second.

**The control map.** Out of the box the map is the PTZ Pro's layout:

```sh
ptz-panel map                          # the map, against the published PTZ Pro layout
ptz-panel map --panel 192.168.0.60     # the map, against what that panel says it has
```

Each row is a role (`estop`, `go`, `key1`, `joy_pan`, ...), what it does, and
the control it is bound to. With `--panel` the program asks the panel for its
controls and lists every role that will not work there: a control the panel
does not have, a button where a knob is needed, a soft key with no display.

To change a binding, press the control you want:

```sh
ptz-panel learn --panel 192.168.0.60                # every role in turn
ptz-panel learn --panel 192.168.0.60 estop joy_tilt # only these
```

For each role it says what to do on the panel (press it, turn it, push the
joystick fully to the right). Enter keeps the binding, `-` removes it, `q`
stops. A joystick axis pushed the other way is marked as inverted. The result
is written to `panel.json`. You can also write the file by hand; it needs only
what differs from the PTZ Pro layout:

```json
{
  "roles": { "estop": 50, "zoom_rocker": 60, "page_up": null },
  "invert": ["joy_tilt"]
}
```

`ptz-panel` reads `--map`, or `panel.json` in the folder it is started in.
`ptz-server --panel` reads `panel.json` in the data folder. With no file the
PTZ Pro layout is used. A file that cannot be read stops the program with the
reason, rather than running a panel with the wrong map.

**Program mode's preset range.** In Program mode (U4 and Image) a camera's
number stores where its rig stands, or uses a preset that camera already has
when the pose is near enough to one. How near is set in Show settings on the
**Programming** page: a range for each of pan, tilt, zoom, focus and
elevation, and a box to switch the looking-up off. It is saved with the show.
The [Programming Manual](programming-manual.md#program-marking-moves-against-the-song)
has the whole of Program mode.

**Without a panel.** `ptz-panel fake` is a stand-in PTZ Pro in a terminal. It
prints what each display shows and takes typed commands (`press 6`,
`hold 9 1.2`, `joy 1 400`, `turn 5 -3`). Start it, then point the server or
`ptz-panel` at it:

```sh
ptz-panel fake                         # terminal 1: listens on 127.0.0.1:9923
ptz-panel --panel 127.0.0.1            # terminal 2
```

**What happens when things drop**

- *The panel is lost* (power, cable, switch). Hand control stops: at once when
  the connection closes, and within about half a second when the link just
  goes silent. While anything is deflected the panel is asked for an answer
  every 100 ms, and after 350 ms without one the rig is let go of; against the
  stand-in panel the camera stood still 0.35 to 0.5 s after the link went
  silent, and the test fails above 0.6 s. A real panel has not been measured. A song that is playing keeps playing. The panel keeps the lease for
  30 seconds (`--lease-grace`) and then gives it back. When the panel returns,
  every display is drawn again, the lease is not taken back by itself, and the
  joystick must come to rest before it drives again.
- *The server is lost.* The panel shows **No server** and every key goes dark.
  E-Stop cannot reach the rig from the panel until the server answers, and the
  panel says so. A press is kept and sent for ten seconds in case it does.
- *Another station takes the lease.* The panel shows who holds it and goes back
  to watching. E-Stop still works.

**E-Stop on the panel is a network command**, like E-STOP in the app. It goes
from the panel to `ptz-panel`, from there to the server, and from the server to
the rigs. If any of those links is down it does not arrive. It is not a
substitute for the AutoPod+'s own stop control or the power.

#### First time on a real panel

Do this with every real rig disarmed. Better still, do it on a scratch data
folder with no real rig in it (`ptz-server --data /tmp/panel-trial`), so that
everything below moves only the simulator. Keep the app open beside the panel
to see what the server sees.

- [ ] `ptz-panel map --panel <address>` connects, names the panel's model, and
      ends with *The map fits this panel*. If it lists problems, the panel is
      not laid out as the published PTZ Pro: run `ptz-panel learn`.
- [ ] `ptz-panel --panel <address>` says *panel connected* and stays connected
      for ten minutes with nothing touched.
- [ ] CAM 9 reads **E-STOP / Press** in red. The seven mode buttons light, and
      pressing each one changes the soft keys.
- [ ] Every soft-key display is readable, and so is a long message on the big
      display. Note anything cut off or in the wrong place.
- [ ] Show mode, **Take** on CAM 1: the app's header names *Panel* as the
      holder.
- [ ] The Focus knob moves the playhead one frame a click, in the direction
      you expect.
- [ ] Camera mode: joystick right pans right in the app's monitor, up tilts
      up, a clockwise twist zooms in. Let go: it stops, and does not creep
      with the stick at rest. A wrong direction is fixed with `"invert"` or
      `ptz-panel learn`.
- [ ] Driving is smooth. If the camera stutters with the cable in, or the
      display says the panel *has not answered a ping*, the real panel answers
      more slowly than the stand-in: stop here and report it.
- [ ] Press CAM 9 once: the app shows *E-STOP LATCHED*. Tap it: nothing. Hold
      it a second and a half: it resets.
- [ ] Transport mode: tap **Arm**: nothing happens and the display says to
      hold. Hold it one second: the simulator arms. **Disarm**.
- [ ] Hold U4 and press Image: the soft keys read **Back**, **Play**,
      **Forward**. A tap on Forward moves the playhead five seconds; holding
      it winds, and it stops the moment you let go. Pull the panel's cable
      while holding it: the playhead stops within about half a second.
- [ ] Still in Program, press P1: its key flashes amber and the app's preset
      list has a new preset. Tap Forward, press P1 again: a move appears on
      camera 1's lane in the app. **Undo**.
- [ ] Push the joystick and pull the panel's network cable: the camera in the
      app's monitor stops within about half a second. Plug it back in: the displays
      return and nothing moves until the stick has been let go and pushed
      again.
- [ ] With the joystick pushed, stop `ptz-panel` with Ctrl-C: the camera stops.

Only then use the panel with a real rig, and the first few times with someone
at the app's E-STOP.

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
| `state.json` | Which song is open |
| `songs/<id>/` | Each song: the working copy, the last Save, every committed revision (`rev/`), and recorded passes (`passes/`) |
| `media/<id>/` | Each imported file, its decoded audio, its waveform, and a video's frame pictures |
| `stage/model.ptzmesh` | The imported stage model |
| `log/<date>.jsonl` | The run log |
| `panel.json` | Only if you made one: what differs from the PTZ Pro layout for a SKAARHOJ panel run with `ptz-server --panel` |

Every file is written to a temporary name and then renamed, so a crash or a
power cut never leaves a half-written song.

The app keeps only its own window settings (server address, recent servers,
station name, access key, panel layout): on a Mac in
`~/Library/Application Support/PTZ Programmer/settings.json`, on Linux in
`~/.config/PTZ Programmer/settings.json`. Setting the `PTZ_APP_DIR`
environment variable before starting the app keeps that file in a folder of
your choosing instead, which lets two stations run on one machine.

**Passwords and keys.** The FR7 web password is kept in `project.json` and is
sent to every station that connects; the access key is kept in the app's
settings file and, on the server, comes from `--access-key` or the
`PTZ_ACCESS_KEY` environment variable. The server and the app talk plain HTTP.
Keep the show on its own closed network, and do not reuse either secret
anywhere that matters.

### Back up and move a show

Stop the server and copy the whole data folder. To restore it, or to move the
show to another machine, copy it back and start the server with `--data`
pointing at it.

### The run log

Everything the server does to the show is written to the run log and shown in
the **Log** tab: each transport action and who asked for it, each refusal and
its reason, what was armed (song, revision and its SHA-256), the rig's pose and
error once a second while live, faults, how long an E-Stop took, devices
connecting and dropping, a hardware panel connecting and going. Each record carries the hash of the one before it:

```sh
ptz-server --verify-log
```

walks the chain and names the first record that was changed, or removed from
the middle. Records cut off the end of the newest file cannot be detected this
way; copy the log off the machine after a show if it matters.

### Update

A copy of the Mac app that came from a published release updates itself. It
looks for a newer version when it opens and every six hours, and says when
there is one. Then:

1. Stop the show, disarm, and turn Record off. The app and the show server on
   that Mac close during the update.
2. **View → Update to <version>**, then **Install and reopen**.
3. Type the Mac's password when it asks.

The app installs the new version and reopens, and restarts the show server on
that Mac as the new version. Do each station in turn, the server's Mac first.
The data folder is not touched, and a project saved by an earlier version is
brought up to date when the server opens it. The Mac needs to reach the
internet for this; **View → Updates** says why if it cannot.

A copy built from source does not look for updates. To update one, back up the
data folder, quit the app, stop the server, pull the new source, then build and
run the installers again (or `make install-mac`, if that is how it was
installed).

### Remove

How to remove it depends on how it was installed. In every case the data
folder and the app's settings file stay until you delete them.

| Installed with | Remove with |
| --- | --- |
| The Mac installers | `sudo rm -rf "/Applications/Motion Choreographer.app" /usr/local/ptz-programmer /etc/paths.d/ptz-programmer`, then `sudo pkgutil --forget com.ptzprogrammer.app` and `sudo pkgutil --forget com.ptzprogrammer.server` |
| The Linux installers | The same installer with `--uninstall` (the app installer also takes `--with-server` to remove the server with it) |
| `make install-mac` | `make uninstall-mac` |

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
| No sound, but the meters move | The output is routed to another channel or device | Check **Audio output** in Show settings and the device named on the bar |
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
| `ptz-panel`: *No panel answers at ...* | Nothing is listening on the panel's address and port | Check the address, the cable, and that the panel is in Raw Panel mode |
| `ptz-panel`: *... took the connection but said nothing* | Something answers, but not as a Raw Panel | Switch on Raw Panel mode on the panel; make sure nothing else is connected to it |
| The panel's display: *Panel map: ...* | A role is bound to a control this panel does not have, or of the wrong kind | `ptz-panel map --panel <address>` lists them; `ptz-panel learn` fixes them |
| The panel: **No server** | `ptz-panel` cannot reach the show server | Start the server, or check `--server` and `--access-key`. E-Stop on the panel does not reach the rig until it answers. |
| The panel: *Let the joystick come to rest, then drive again* | The joystick was already pushed when it was given the rig, or the panel or the server had been away | Let go, then push again |
| The panel: *The panel stopped answering during the hold. Nothing was done* | The link to the panel went quiet while a key was being held | Check the panel's cable and switch, then hold again |
| The panel: *This panel has not answered a ping ...* | The panel does not answer the link check, so driving by hand from it is kept off | Report it: the real panel behaves differently from the published protocol |
