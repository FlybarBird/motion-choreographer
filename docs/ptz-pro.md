# PTZ Pro: setup and programming

How to put a SKAARHOJ PTZ Pro on the show, and how to program songs from it.
This is the manual to keep at the panel. Installing the software, building the
stage model and bringing up each FR7 and AutoPod+ are in the
[Setup Guide](setup-guide.md). The app's timeline, graph editor and revisions
are in the [Programming Manual](programming-manual.md).

> This has been tested against a stand-in panel and SKAARHOJ's published
> layout, not yet on a real PTZ Pro, and not yet with two real controllers on
> one real FR7. The first time, do it with every rig disarmed, and with the
> app open beside the panel.

| | |
| --- | --- |
| [1. What the panel is](#1-what-the-panel-is) | It programs the timeline. It is not the camera's controller. |
| [2. Set the show up first](#2-set-the-show-up-first) | Frame rate, cameras, rigs, timecode, the preset range |
| [3. Connect the panel](#3-connect-the-panel) | Raw Panel mode, the address, what "connected" looks like |
| [4. The controls](#4-the-controls) | Where everything is, and how the lights read |
| [5. Program a song](#5-program-a-song) | The fast way: play, frame on your own controller, two presses |
| [6. Tidy, save and commit](#6-tidy-save-and-commit) | Length, ease, problems, Save, Commit |
| [7. Rehearse and go live](#7-rehearse-and-go-live) | CAM 8, Arm, Play Live, E-Stop |
| [8. The other modes](#8-the-other-modes) | Every soft key, when you are not in Program |
| [9. When it will not](#9-when-it-will-not) | The messages, and a lost cable |

---

## 1. What the panel is

You already have a controller for the cameras: the FR7's remote, a PTZ
controller, the AutoPod+'s own panel. Leave that job there. The PTZ Pro is
another operator station. It holds the edit lease under the name *Panel*,
moves the playhead, reads where a camera already stands, and writes the move.

Three things, often on fewer machines:

| | Where it runs | What it owns |
| --- | --- | --- |
| The show server | One computer that stays on | The songs, the cameras, the audio, the timecode |
| The app | That computer, or another one | Setup, and a window on what the panel is doing |
| The PTZ Pro | The panel's own network port | Programming and, later, running the show |

Timecode and audio are plugged into the **server computer**. The app never
hears timecode itself. A station that only runs the app shows what the server
is locked to.

One station at a time can change the show. If the app holds the lease, the
panel watches, and the other way round. E-Stop works from either, with or
without the lease.

---

## 2. Set the show up first

Do this in the app, once, before the panel is any use. Parts 1 to 7 of the
[Setup Guide](setup-guide.md) are the long version. The short version:

1. Start the server and open the app. **Take the lease.**
2. Open **Show settings** at the right end of the camera bar.

**Frame rate**, on *Frame rate and faults*. Choose it once, before any song,
and match the timecode the show will lock to: 23.976, 24, 25, 29.97, 29.97df
or 30. **29.97** is non-drop: thirty numbers a second and none skipped, so an
hour of code lasts 3.6 seconds longer than an hour on the clock. **29.97df**
skips frame numbers so the clock and the code agree. Timecode itself cannot
tell 29.97 from 30, or 23.976 from 24. The project's rate decides. Pick the
one the source really runs at.

**Cameras**, on *Cameras*. Name them, and put them in the order of the panel's
number keys. Camera 1 is **P1**, camera 2 is **P2**, and so on up to **P10**.
Cameras 11 and 12 are Shift and P1, Shift and P2. Add and remove cameras only
while the show is stopped and disarmed.

**Each real camera**, on *Rig connection*. Set **Drives** to *FR7 and AutoPod+*
and give the addresses. Program mode reads the pose the rig is already in. If
the rig is not answering, a press is refused with *The rig's pose is not
known*. Until the rig is connected, that camera is the simulator.

**How near is the same preset**, on *Programming*. When you press a camera's
number, the server looks for a preset that camera already has. If every axis
is inside these ranges, it uses the nearest one and stores nothing new. Out of
the box: 1 degree of pan, 1 degree of tilt, 2% of zoom, 5% of focus, 10 mm of
height. Wider ranges make fewer near-duplicate presets. Clear **Use a preset
within range** and every press stores a new one. Presets you store from the
app's own Store button are not looked up this way.

**Sound and timecode**, on *Audio output* and *Timecode inputs*. Both pages
list devices on the **server machine**, whichever computer the app is open on.
LTC in is an audio input there, MTC is a MIDI port there, Art-Net listens on
the server (`0.0.0.0:6454` is every interface). LTC out is a channel of the
server's audio output.

Limits, keep-out and the stage model can wait until you rehearse. They are
what the monitors and the "too fast" warnings are built from, so set them
before a live move. That work is [Part 5](setup-guide.md#5-the-venue-stage-model-and-camera-positions)
and [Part 7](setup-guide.md#7-the-real-rig) of the Setup Guide.

---

## 3. Connect the panel

### On the network

Give the panel a fixed address on the same wired network as the server. The
server connects to the panel. The panel does not connect to the server. The
port is **9923** unless you change it on the panel.

Nothing else may drive the panel at the same time. Raw Panel mode and the
panel's own Reactor program cannot both run.

### On the panel

SKAARHOJ's own pages describe this; it has not been checked here on a real
panel. Their system manager needs to be 1.0.6 or newer.

1. Open the panel's web page.
2. On Settings, switch on **Raw Panel Mode**.
3. Accept the offer to switch Reactor off.
4. Leave the port at 9923, or write down what you change it to.

### In the app

**Show settings**, then **Control panel**, under *The server*.

1. **Panel address**: the panel's address, such as `192.168.0.60`. Add
   `:port` only if it is not 9923.
2. Tick **Use this panel**. The box stays off until there is an address.
3. Read the line under it.

| It says | Meaning |
| --- | --- |
| Connected to … It is the station "Panel". | The panel is on the show. CAM 9 reads **E-STOP / Press** in red, and the top left display shows the song and the timecode. |
| Looking for the panel at … | It is off, on another network, or not in Raw Panel mode. The server keeps trying. |
| The server could not start the panel: … | The address or the control map is wrong. It tries again every few seconds. |
| This server was started with a panel of its own | The address was given on the command line. Change it there, not here. |

You need the edit lease, and the show must be disarmed, to change this. The
address is saved with the show, so the server connects to the panel every time
it starts.

On connection and reconnection, the controller disables the panel's inactivity
sleep timer and wakes the displays. Network heartbeats only check the connection;
they do not prevent screen sleep. If the panel reports that it has gone to sleep,
the controller releases hand control, wakes it and redraws the controls. This
behavior is tested against the stand-in panel and still needs a real-panel check.

The other ways, if you would rather not use the page:

```sh
ptz-server --panel 192.168.0.60
ptz-panel --panel 192.168.0.60 --server http://192.168.0.10:8787 --access-key "the-key"
```

`--panel` on the server wins over the page. Stopping `ptz-panel` with Ctrl-C
does not stop the show: hand control stops, the lease is given back, and the
keys go dark.

### First time, with nothing at risk

Do this on the simulator (`ptz-server --data /tmp/panel-trial` is a scratch
show), with the app open so you can see what the server sees.

- [ ] `ptz-panel map --panel <address>` names the model and ends with *The map
      fits this panel*. If it lists problems, the panel is not the published
      PTZ Pro layout: run `ptz-panel learn --panel <address>`.
- [ ] CAM 9 reads **E-STOP / Press**. Press each of the seven mode buttons.
      The soft keys change, and each display is readable.
- [ ] Show mode, **Take** on CAM 1. The app's header names *Panel*.
- [ ] The Playhead knob moves the timecode one frame a click, in the direction
      you expect. A fast turn covers more. With Shift, a click is one second.
- [ ] Camera mode: joystick right pans right in the monitor, up tilts up, a
      clockwise twist zooms in. Let go and it stops, with no creep. A wrong
      direction is `"invert"` in `panel.json`, or `ptz-panel learn`.
- [ ] Press CAM 9 once. The app shows E-Stop latched. A tap does not reset it.
      Hold it a second and a half and it resets.
- [ ] Hold U4 and press Image. The soft keys read **Back**, **Play**,
      **Forward**. Tap Forward: five seconds. Hold it: it winds, and it stops
      the moment you let go.

The full list, including pulling the cable, is
[First time on a real panel](setup-guide.md#first-time-on-a-real-panel).

Without a panel, `ptz-panel fake` is the same layout in a terminal (`press 6`,
`hold 9 1.2`, `joy 1 400`). Point `ptz-panel --panel 127.0.0.1` at it.

---

## 4. The controls

The printed names are the camera's. This program uses them as follows.

| On the panel | In this program |
| --- | --- |
| **EXP, WB, Color, Image, U1, U2, U3** | The mode: Show, Transport, Camera, Presets, Timeline, Record, Media. The lit one is where you are. |
| **U4 held, then Image** | **Program**, the mode this manual is mostly about. Image stays lit. |
| **U4** | Shift. Hold it. It changes what the soft keys do, and it makes knobs step further. |
| **CAM 1 to CAM 7** | Soft keys. The display on each one says what it does right now. |
| **CAM 8** | The main transport button, in every mode: Rehearse, Play to code, Play Live, Stop, or Reset E-Stop. The same next step as the app's main button. |
| **CAM 9** | **E-Stop**, in every mode. |
| **1 to 10** | Select camera 1 to 10. In Program they mark moves instead of only selecting. |
| **Up, Down** | The previous and next page of a list. |
| **Playhead** (the Focus knob) | The playhead. One frame a click. Turn it fast and it covers more. With Shift, a click is one second. |
| **Knobs 1 to 4** | Four values for the mode, named on the displays above them. |
| **Joystick** | The selected camera, in Camera, Presets, Record and Program. Left and right pan, up and down tilt, twist zooms. It stops when you let go. |
| **Status** (top left display) | The song, with `*` when it is unsaved, then the timecode and the transport. It also pages through the mode and camera, who holds the lease, saved or committed, problems, and lock. |
| **Messages** (top right display) | What was just done, why something was refused, what a hold will do, a question waiting for Confirm. A long message is shown a page at a time, in full. |

A dim key is available. A bright key is also showing state: red for Record and
for deleting, amber for something to attend to, green for go, blue for what is
selected. A dark key can do nothing right now. Press it and the message
display says why.

Choosing a mode or a camera never moves anything.

Some keys say **Hold**. A tap does nothing except say what the hold would do.
Keep it down until the display changes. Let go early and nothing happens. A
hold counts only while the panel is still answering: if the cable comes out
during a hold, nothing is done.

| Hold | For |
| --- | --- |
| **Arm**, and **Arm** when CAM 8 offers it | 1 second |
| **Play Live** and **Play to code** while armed | 1 second |
| **Reset E-Stop** | 1.5 seconds |
| **Take over** another station's lease | 1 second |

Three things ask first: **Revert**, deleting a preset, and **Sync from rig**
on a real camera, because that one moves the camera to each preset. CAM 1
becomes **Confirm** and CAM 7 **Cancel**. No answer in ten seconds is Cancel.

---

## 5. Program a song

Program treats the timeline as a tape. You get the playhead to a time, frame
the camera on your own controller, and press that camera's number twice: once
for where it is, once for when it must be there.

The show has to be **disarmed**. Armed, moving the playhead moves the rig, so
every Program key is dark and says *Disarm to program*. Disarmed, playing and
winding move no real camera. The camera stays where your controller left it,
which is what the next press reads.

### Open it

1. **EXP** (Show). CAM 1 **Take**, if the key says Take.
2. CAM 2 **New**. The song is *Song N*, starting on the next ten minutes of
   timecode after the latest song. Rename it in the app.
3. Hold **U4** and press **Image**. The soft keys change to the tape, and
   Image stays lit.

| Key | Tap | Hold |
| --- | --- | --- |
| CAM 1 **Back** | Five seconds back | Winds back, at 8 times speed, then 24 times after three seconds |
| CAM 2 | **Play** from the playhead. Press again: **Pause**, and the playhead stays | |
| CAM 3 **Forward** | Five seconds on | Winds forward, same speeds |
| CAM 4 **To start** | The playhead to the start of the song | |
| CAM 5 **Cancel** | Takes back the newest mark. Dark when nothing is marked | |
| CAM 6 **Save** | Saves every song with unsaved changes. Amber when the loaded song has something to save | |
| CAM 7 **Undo** | Undo. With U4 held, **Redo** | |

With **U4** held, Back and Forward say **1 frame**. A tap is one frame. A hold
steps frame by frame, about ten a second. A key let go within a third of a
second is a tap. You may press the camera's number with U4 still down: it is
still that camera, unless the show has eleven or twelve cameras, in which case
P1 and P2 are cameras 11 and 12.

Winding stops when you let go, at either end of the song, and if the panel
stops answering. It is refused while the show is locked to timecode. Unlock
first (Transport, CAM 6).

The Playhead knob works here too.

### Mark one move

1. Get the playhead to where this camera should **leave** for the new framing.
   Play and pause, or tap in fives, or hold to wind, or step frames with U4.
   Pause before you frame the camera.
2. On the camera's own controller, set pan, tilt, zoom, focus and the
   AutoPod+'s height. The PTZ Pro does not move a real camera. (On the
   simulator there is no other controller: use this panel's joystick, and
   knobs 1 to 3 for focus, elevation and top speed. Set the framing **after**
   the playhead is in the right place. The simulator shows the camera where
   the song has it whenever the playhead moves.)
3. Press the camera's number, **1** to **10**. The server reads where the rig
   stands. The key **flashes amber**. The message display says one of:
   - *Cam 2 marked at 01:00:05:00 as new Preset 3.* A new preset was stored.
   - *Cam 2 is within range of Wide: using it.* An existing preset was close
     enough on every axis. Nothing new was stored, and the move will go to
     that preset, not to the slightly different place the camera stood.
4. Play, wind or step to where the camera must **have arrived**. Press the
   same number again.

The move is on that camera's lane. It leaves at the first press and has
arrived at the second, eased in and out. The key stops flashing. The display
gives both times and the length, for example *Cam 2 moves to Wide from
01:00:05:00 to 01:00:18:00, 13.00 s*.

A second press at the same time, or earlier, does not make a backwards move.
It places a move at the playhead that takes its auto time, as Presets mode
does.

### Several cameras, and a run of shots

You can mark more than one camera before you move on. At a verse, pause, set
camera 1 and press **1**, set camera 3 and press **3**. Both keys flash. Play,
and press each number on the beat where that camera must have arrived. The
message display lists the marks that are still open.

Then do the next shot the same way. A framing you have used before, within the
range on the Programming page, comes back as the preset you already named
instead of *Preset 14*.

### A mark you do not want

**Cancel** (CAM 5) drops the newest mark.

- If that press stored a new preset, the preset is removed with the mark.
- If that press reused a preset, the preset is kept.

Marks are also dropped when another song is loaded, and when the panel loses
the lease. The presets they stored stay.

**Undo** (CAM 7) takes back the last edit, including a move you just placed.
It remembers what this panel has done since the song was loaded, up to two
hundred steps. It does not remember the app's edits, and the app's undo does
not remember the panel's.

### When the second press is refused or unhappy

| The display says | What happened | What to do |
| --- | --- | --- |
| **PROBLEM** … too fast | The move was placed, and the two presses are closer than the rig can travel | Undo, mark it longer, or fix it in Timeline. You can also accept it there. |
| Overlaps another move | Nothing was placed. The key keeps flashing | Go to a time that camera is free and press again, or Cancel |
| The rig's pose is not known | The first press could not read the camera | *Rig connection* in Show settings. The rig has to be answering. |
| Disarm to program | The show is armed | Transport, **Disarm**. Programming while armed would move the rig. |
| No camera is marked | Cancel with nothing marked | Press a camera's number first |

---

## 6. Tidy, save and commit

Program gets the moves onto the lanes quickly. Timeline is where you shape
them. Press **U1**.

| Key | Does | With U4 |
| --- | --- | --- |
| CAM 1, CAM 2 | **Prev** and **Next** clip on this camera. The playhead goes to it | Playhead **To start**, **To end** |
| CAM 3 | **Ease**: step through the eases | **To keys**: turn the move into curves |
| CAM 4 | **Auto time**. Amber when that would clear a "too fast" | **Accept as is**, or **Withdraw accept** |
| CAM 5 | **Duplicate** | **Add** a marker at the playhead |
| CAM 6 | **Delete** the clip | **Delete** the marker at the playhead |
| CAM 7 | **Undo** | **Redo** |

Knob 1 slides the selected clip. Knob 2 changes its length, which switches it
from auto time to a time you chose. Knob 3 jumps to the next or previous clip
edge. Knob 4 jumps between markers, and a push adds one.

The status display counts problems. A speed or acceleration problem can be
accepted. A move that leaves the rig's range, jumps, or enters a keep-out
cannot: change the move.

To point a move at a different preset without rebuilding it, use the app: drag
that preset onto the move, or set **To** in the Inspector. The time stays.
Dragging a preset into a gap still makes a new move.

**Save** often. In Program it is CAM 6. In Show it is CAM 4. It is amber while
there is something unsaved, and the status display puts `*` on the song name.
In the app, **Cmd-S** and **Ctrl-S** both save.

**Commit** (Show, CAM 5) freezes a revision. Only a committed revision can be
armed. Commit is amber when the saved song is not yet a revision. Save first.

---

## 7. Rehearse and go live

**CAM 8** is the transport in every mode, so a rehearsal is CAM 8 to play and
CAM 8 to stop. Rehearse plays the audio and the monitors. It sends nothing to
a rig.

Going live is Transport (**WB**):

1. Clear the area. A committed revision has to be loaded.
2. Hold **Arm** (CAM 4) for one second. A tap only tells you to hold. The key
   reads **Arming** while the rigs travel to the playhead, then **Armed**.
3. On the cue, hold **Play Live** (CAM 8, or CAM 1) for one second.
4. **Stop**, then **Disarm**.

**CAM 9** stops every camera, in any mode, with or without the lease, even
while a question is on the display. The key then blinks **Hold: reset**. Let
go. The press that stopped the show never resets it. To reset, the panel must
hold the lease: hold CAM 9 for a second and a half.

E-Stop is a network command. It goes from the panel to the server, and from
the server to the rigs. If any of that path is down, it does not arrive. It is
not a substitute for the AutoPod+'s own stop or for the power.

Program stays dark until you Disarm. That is deliberate.

---

## 8. The other modes

Use these when you are not marking against a tape. The soft key always says
what it does. This is the whole set.

### Show (EXP): the lease and the songs

| Key | Does |
| --- | --- |
| CAM 1 | **Take** the lease, **Release** it, or **Take over** (hold) from the station named on the key |
| CAM 2 | **New** song |
| CAM 3 | **Browse**. Songs on CAM 1 to CAM 6, the loaded one lit, Up and Down for more, CAM 7 **Back** |
| CAM 4 | **Save** |
| CAM 5 | **Commit** |
| CAM 6 | **Save copy**, under a new numbered name |
| CAM 7 | **Revert** to the last Save. It asks first |

### Camera (Color): frame it here, when you have no other controller

CAM 1 to CAM 6 are the cameras, six to a page. The joystick drives the
selected one. Knob 1 is focus, knob 2 is elevation, knob 3 is top speed, from
5% to 100% of each axis. CAM 7 **Store** saves where it is as *Preset N*.

In Rehearse, this drives the simulator. To frame a real FR7 in Rehearse, use
its own controller and press Store, or use Program to read its pose.

While the show is live, the joystick takes manual control of the selected
camera. Release it to return an armed camera to the current show position.
The other cameras and show clock keep running. If the stick is already
deflected when you enter the mode or change camera, the display says *Let the
joystick come to rest, then drive again*.

### Presets (Image): place a move at the playhead

| Key | Does | With U4 |
| --- | --- | --- |
| CAM 1 | **Store** a new preset | **Sync from rig**. Asks first on a real camera, because the camera moves |
| CAM 2 to CAM 6 | Place a move to that preset at the playhead. It takes its auto time | **Delete** that preset. Asks first |
| CAM 7 | **Undo** | **Redo** |

Up and Down page through more than five presets. Put the playhead where the
move should start, then press the preset.

### Record (U2): perform a move

1. CAM 1 to CAM 5 are the axes that will be written. Red is on. Choose them
   while stopped.
2. CAM 6 **Record** on.
3. CAM 8 to play, and drive.
4. CAM 8 to stop. The pass lands as a keys clip, selected, and Record turns
   off. Undo removes it. A camera that was not moved during the pass is not
   written.
5. With U4, CAM 1 **Fewer keys** and CAM 2 **More keys** thin or thicken the
   selected pass.

A keys clip is a performance, not a preset. Program will not replace one. Drop
a preset beside it, or edit the curves in the app.

### Media (U3)

Files are imported in the app. CAM 1 to CAM 4 place one at the playhead. Up
and Down page them. CAM 5 selects a media clip, CAM 6 mutes it. With U4, CAM 5
takes it off the timeline and CAM 6 locks the song to that file's timecode, or
unlocks.

### Transport (WB)

| Key | Does |
| --- | --- |
| CAM 1 | **Rehearse**, **Play to code** or **Play Live** (hold while armed) |
| CAM 2 | **Stop** |
| CAM 3 | **Cue** to the start |
| CAM 4 | **Arm** (hold), then **Armed** |
| CAM 5 | **Disarm** |
| CAM 6 | **Lock to** the source named on the key. Knob 1 chooses the source. **Unlock** when locked |
| CAM 7 | **Record** on and off |

---

## 9. When it will not

The message display quotes the server. These are the ones that come up while
programming.

| You see | Why |
| --- | --- |
| Watching. Take the edit lease | Someone else holds it, or nobody does. Show, CAM 1. Taking over from the app is a hold. |
| The panel no longer holds the edit lease: the marks are dropped | The app, or another station, took the lease. Presets already stored are kept. The flashing keys go out. Take it back and mark again. |
| Create or load a song first | Show, CAM 2 or CAM 3. |
| Unlock to cue | The playhead is following timecode, so Back, Forward and the Playhead knob will not move it. |
| No server | The panel cannot see the server. Every key goes dark. E-Stop cannot reach the rig from here until the server answers, and a press is kept and sent for ten seconds in case it does. |
| Let the joystick come to rest, then drive again | The stick was over when the panel was given the camera. Centre it, then push. |
| Press Camera to drive by hand | The joystick does nothing in Show, Transport, Timeline and Media. |

A lost panel does not stop a song that is playing. Hand control stops at once
if the connection closes, and within about half a second if the cable is
pulled and the link just goes quiet. The panel keeps the lease for 30 seconds
and then gives it back. When it returns, the displays are drawn again, the
lease is not taken back by itself, and the joystick has to be centred before
it drives. A wind that was in progress stays stopped.

### Still done in the app

Typing names. The graph editor and single keys. Cameras, lenses, rigs, limits,
keep-out, the stage, audio and timecode. Importing media. Any revision other
than the latest. The run log. Replacing where a move goes by dragging a preset
onto it.

The app can stay open on another computer while the panel holds the lease. It
watches, until someone there takes over.

---

## A song, start to finish

1. Show: **Take**, **New**.
2. U4 and Image, for Program.
3. For each shot: pause where the camera leaves, frame it on its own
   controller, press its number, go to where it must have arrived, press the
   number again. Mark several cameras at one pause when they leave together.
4. U1, Timeline: **Next** through the moves. Ease, length, auto time. Fix or
   accept anything the status display counts.
5. CAM 8 to rehearse. CAM 8 to stop. U4 and **To start** to go back.
6. Show: **Save**, **Commit**.
7. Clear the area. Transport: hold **Arm**. On the cue, hold **Play Live**.
   **Stop**, then **Disarm**.

If anything looks wrong: CAM 9.
