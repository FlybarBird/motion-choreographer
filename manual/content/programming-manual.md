# Motion Choreographer: Programming Manual

How to program a show: build songs out of camera moves, shape them, rehearse
them, and play them live. It assumes the system has been set up as described in
the [Setup Guide](setup-guide.md): the server is running, the cameras, lenses
and venue are in the project, and (for a live show) each rig has been through
bring-up. If you program from a SKAARHOJ PTZ Pro, use
[PTZ Pro: setup and programming](ptz-pro.md); chapter 14 is the key-by-key
list.

Everything in this manual up to [Going live](#12-going-live) can be done on the
simulator with no hardware connected.

| Chapter | |
| --- | --- |
| [1. How it thinks](#1-how-it-thinks) | The ideas the rest depends on |
| [2. The window](#2-the-window) | What is where |
| [3. Your first song](#3-your-first-song) | Ten minutes, start to finish |
| [4. Presets](#4-presets) | The poses moves are made from |
| [5. The timeline](#5-the-timeline) | Placing, moving, timing and selecting |
| [6. Issues](#6-issues) | When a move asks for something the rig cannot do |
| [7. Keys and the graph editor](#7-keys-and-the-graph-editor) | Shaping a move by hand |
| [8. Record](#8-record) | Performing a move and keeping it |
| [9. Several cameras](#9-several-cameras) | Programming up to twelve |
| [10. Audio, reference video and timecode](#10-audio-reference-video-and-timecode) | What the song plays against |
| [11. Save, Commit and revisions](#11-save-commit-and-revisions) | Keeping your work, and what can be armed |
| [12. Going live](#12-going-live) | Arm, Play Live, faults, E-Stop |
| [13. A show day](#13-a-show-day) | The routine |
| [14. Programming from a panel](#14-programming-from-a-panel) | The same work on a SKAARHOJ PTZ Pro. Setup and a song from nothing are in the [PTZ Pro manual](ptz-pro.md) |
| [15. Reference](#15-reference) | Shortcuts, chips, messages, glossary |

---

## 1. How it thinks

**A song** is one timeline: a piece of the show with a start timecode and a
length. A show is a set of songs. The server holds one song open at a time.

**A preset** is a stored pose for one camera: pan, tilt, zoom, focus and
elevation. Presets are the vocabulary; a song is what you say with them.

**A preset move** is a clip on a camera's lane that takes the camera *to* a
preset. Unless you pin its start to a preset, it does not store where it
starts: a move starts from wherever the camera already is when the move
begins, which is wherever the previous move left it. Slide a move later and it
still arrives at the same pose.

**Auto time.** A new move is exactly as long as the rig needs: the software
works out the fastest time from the distance between the two poses and the
rig's speed and acceleration limits. Stretch a move and it becomes **custom
time**. Squeeze it shorter than the rig can manage and it becomes an **issue**.

**Keys.** When a move needs a shape a preset cannot give it (a pause, a curve,
a zoom that starts late), it is converted to keys: one curve per axis, edited
in the graph editor or recorded by hand.

**Rehearse and Live are different outputs.** Rehearse plays the audio and the
monitors and sends nothing to the rig, ever. To move hardware you **Arm**, and
only a **committed** revision of a song can be armed. That is the gate between
editing and a show: what goes live is a frozen revision, not whatever was on
screen a moment ago.

**The edit lease.** One station at a time can change the show. Others watch.
E-Stop is the exception: it works from anywhere.

**The selected camera.** One camera is selected at any time. The Program
monitor, the Presets tab, the Rig controller and Record all follow it.

---

## 2. The window

**Header** (top row): the song commands (New, Load, Save, Save As, Revert,
Commit), the song's name and start timecode, its revision state, the issue
count, and at the right the edit lease and the View menu.

**Transport bar** (second row): the timecode at the playhead, the main button,
Stop, Cue, Lock, Arm or Disarm, Record, the state chips, a one-line hint, the
audio meters, and E-STOP at the far right.

**Camera bar** (third row): one chip per camera in its own colour, with a dot
for its state. Click a chip to select that camera. Hover it for detail. At its
right end is **Show settings**.

**Tabs.** The rest of the window is panels you can drag by their tabs, split
and regroup. **View > Reset layout** puts them back.

**Show settings.** What belongs to the whole show and not to one song (the
cameras, their rigs, lenses, positions, limits and keep-out rules, the frame
rate, the stage model, the audio output and the timecode inputs) is set in one
place. **Show settings** on the camera bar puts it where the panels are;
**Close settings** or Esc brings the panels back. The header, the transport,
E-STOP and the camera bar stay on screen and keep working. While it is open
the keys that edit or move through the song do nothing: only Space, the
camera keys and Cmd-S or Ctrl-S act. The setup guide goes through every page.

| Tab | What it is for |
| --- | --- |
| Presets | The selected camera's presets. Drag one onto the timeline. |
| Set list | The show's songs in running order. Click one to load it. |
| Media | Imported audio and video. Drag one onto the timeline. |
| Program monitor | What the selected camera frames, through its lens, in the stage model |
| Stage | The venue from outside, with every camera and its view cone |
| Multiview | Every camera's picture at once |
| Reference monitor | The frame of the reference video under the playhead |
| Inspector | Whatever is selected: a move, a clip, a marker. With nothing selected, the song. |
| Rig controller | Hand control of the selected camera, its devices, and tracking while live |
| Timeline | The song |
| Graph editor | The selected clip as curves |
| Log | The server's run log |

The Program monitor's corner badges tell you what you are looking at:
**REHEARSE** or **LIVE** at top left; *Programmed pose*, *Rig pose* or
*Recording* at top right; the timecode at bottom left; the camera and the focal
length at bottom right. Under the picture is the pose in numbers.

---

## 3. Your first song

This uses the three presets a new data folder is seeded with. It takes about
ten minutes and touches everything you will use daily.

1. **Take the lease.** If the header says *Watching*, press **Take the lease**.
2. **New.** Press **New** in the header. Name the song and give it the
   timecode its first frame plays at, for example `01:00:00:00`. Press
   **Create**.
3. **Place a move.** Open the **Presets** tab. Drag **Stage Wide** onto the
   timeline and let go about two seconds in. While you drag, a ghost shows
   where the move will land and how long it will take. It lands on the selected
   camera's **Camera move** lane.
4. **Place two more.** Drag **Lectern MCU** to about ten seconds, and **Awards
   Two-Shot** to about twenty. Each ghost names the journey, for example
   *Stage Wide to Lectern MCU, 7.9 s*: the time comes from how far the camera
   has to travel.
5. **Rehearse.** Press **Space**. The playhead runs and the Program monitor
   shows the camera travelling between the three framings. Nothing is sent to
   any rig. Press **Space** again to stop and **Home** to go back to the start.
6. **Slow a move down.** Drag the right edge of the second move to make it
   longer. Its label gains *custom*. Rehearse again and see the difference.
7. **Ask for too much.** Now drag the same edge left until the move is very
   short. It turns amber, and the header shows *1 issue*. Select the move: the
   Inspector says which axis is too fast and by how much. Press **Use auto
   time** and the issue is gone.
8. **Change how it lands.** With the move selected, set **Ease** in the
   Inspector to *Ease out* and rehearse. The move now arrives gently.
9. **Mark a cue.** Put the playhead where the second move starts and press
   **M**. Double-click the marker to name it.
10. **Keep it.** Press **Save** (Cmd-S or Ctrl-S), then **Commit**. The header shows
    *revision 1*. This revision is what could now be armed.

Made a mistake at any point? **Cmd-Z** undoes it, **Shift-Cmd-Z** redoes it.

---

## 4. Presets

Presets belong to a camera. The Presets tab shows the selected camera's
presets, each with a picture of its framing, its pose and its focal length.

| To | Do this |
| --- | --- |
| Make a preset from the pose on screen | **Store current pose** in the Presets tab. It saves the pose in the Program monitor as *Preset N*. |
| Make a preset from where the rig is | Drive the camera in the **Rig controller** tab, then **Store preset** there |
| Read the presets stored in the camera | **Sync from rig**. On a real rig the camera is recalled to each preset in turn, so it moves; the app asks first. Refused while armed or playing. |
| Use a preset | Drag it onto the timeline |
| Delete a preset | Right-click it, **Delete preset** |

A move on the timeline stays linked to the preset it was made from. Storing
over that preset, changing its height, or syncing it from the camera updates
every such move. Deleting a preset does not remove the moves; they keep the
pose they had.

A useful habit: build the presets for each camera position first (wide, each
speaker position, each two-shot), name them for what they frame, and only then
start on the timeline.

---

## 5. The timeline

### The lanes

From the top: the ruler, **Markers**, **Reference video**, **Audio**, then one
**Camera move** lane for each camera, in its colour and with its number. The
triangle on a camera's lane opens its five axis lanes: Pan, Tilt, Zoom, Focus
and Elevation.

### Getting around

| To | Do this |
| --- | --- |
| Move the playhead | Click the ruler |
| Step one frame | Left and Right arrows. With Shift, ten frames. |
| Go to the start, the end | **Home** (Cue), **End** |
| Slide the view | Scroll, or drag the bar along the bottom of the timeline. A click beside its thumb brings it there. |
| Zoom | Pinch, or Cmd-scroll; or **-** and **+** in the timeline's toolbar |
| See the whole song | **Fit**, or `\` |

### Placing and moving

| To | Do this |
| --- | --- |
| Add a move | Drag a preset from the Presets tab. It lands on the Camera move lane at the pointer's time. |
| Set the time for new preset drops | Enter seconds in **Drop time** beside the presets. **Auto** restores timing from the rig limits. The choice is remembered on this station; existing moves keep their time. |
| Change where a move goes | Drag a preset onto that move. Its time stays; the destination changes. |
| Slide a move | Drag it |
| Make it longer or shorter | Drag its right edge. It switches from auto time to custom time. |
| Change when it starts without moving its end | Drag its left edge |
| Give it to another camera | Drag it onto that camera's lane, or change **Camera** in the Inspector |
| Copy and paste | Select clips, markers or keys, press **Cmd-C**, move the playhead, then **Cmd-V**. Use Ctrl on Windows/Linux, or **Copy** / **Paste** in the toolbar or right-click menu. |
| Duplicate it immediately after itself | Alt-drag it, or select it and press **Cmd-D** (**Duplicate**) |
| Delete it | Select it and press **Delete** |

Edges snap to the playhead, to markers and to other clips. Hold **Shift** while
dragging to turn snapping off. **Escape** cancels a drag.

If a drop or a drag is refused (no lease, the song is armed, it would overlap
another move, or the preset is dropped on a recorded pass), the ghost turns
amber and says why.

### Selecting

Click a clip to select it. **Shift-click** adds to the selection. Drag across
empty space to draw a box round several. With several selected: drag to move
them together, **Delete** removes them, **Cmd-D** copies them after the
selection. Click empty space to select nothing, which is also how you get the
song's name, start and length in the Inspector.

Paste keeps the selection's relative spacing, camera lanes, media trims and
durations, and is one undo step. Copied preset moves use Custom time; linked
starts still follow the destination timeline. You can paste into another song
on the same server. After reconnecting or refreshing the media library, copy
media clips again before pasting. Overlaps, missing cameras or media, and different frame
rates are refused without a partial paste. Selected keys can be pasted into
a selected keys clip at the playhead, or become a new clip outside it.

### The Inspector for a move

| Field | Meaning |
| --- | --- |
| Camera | The camera that makes this move |
| From | *Where the camera already is* when the move starts (the default), or a preset the move should start from |
| To | The preset the move arrives at. Pick another to change it. |
| Start, End | In timecode |
| Duration | **Auto**: as fast as the rig's limits allow. **Custom**: the time you set. *Auto time is 7.90 s* tells you the fastest. |
| Ease | Linear, Ease in, Ease out, Ease in-out (the default) |
| Issues | What is wrong with this move, and the buttons that fix it |
| Convert to keys | Replace the move with keys that trace the same path |

Right-click a move for **Auto time** and **Convert to keys**. Right-click empty
space for **Add marker at playhead**.

### Markers

**M**, or **Add marker**, puts a marker at the playhead. Drag a marker to move
it; double-click it to rename it; select it and press **Delete** to remove it.
Clip edges and the playhead snap to markers, which makes them the way to tie
moves to musical or spoken cues: mark the cue while listening, then drop moves
on the marks.

### Timing moves to a cue

A camera should usually *arrive* on the cue, not start on it. Put a marker on
the cue, drag the move so that its **right** edge snaps to the marker, and
leave it on auto time so it leaves as late as the rig allows; or stretch its
left edge earlier for a slower, more deliberate arrival.

---

## 6. Issues

An issue is a place where the timeline asks the rig for something it cannot or
must not do. The clip turns amber, the header counts the issues, and **Arm is
refused until every issue is fixed or accepted.** Click the count in the header
to jump to the first one. The Inspector, with nothing selected, lists them all
under **Issues in this song**, each with **Go there**.

| The issue says | It means | Fix |
| --- | --- | --- |
| *Pan is too fast: 85 per second, limit 60* | The move is shorter than the rig can do it | **Use auto time**, or lengthen the move, or **Accept as programmed** |
| *Tilt starts or stops too sharply* | The acceleration limit is exceeded | Lengthen the move or soften the ease or the curve, or **Accept as programmed** |
| *Pan reaches 190, outside its range* | The pose is beyond the axis's Min or Max | Change the preset or the keys. Cannot be accepted. |
| *Zoom steps by 12 at the start of this move* | The axis would have to jump instantly, usually where keys take over from another clip | Start the keys where the previous move ends, or leave room between them. Cannot be accepted. A camera's first recorded move is never a step: the camera waits where the take starts (chapter 8). |
| *Keep-out rule broken: ...* | Two axes together enter a forbidden combination | Change the move so it stays out. Cannot be accepted. |

**Accept as programmed** is for speed and acceleration only. It says: I know
the rig will lag behind the timeline here, and that is fine. The acceptance
records who accepted it and when, is written to the run log when the song is
armed, and can be withdrawn (**Withdraw acceptance**). Range, jump and keep-out
problems can never be accepted, because they are about where the rig goes, not
how late it gets there.

The limits themselves are in the project settings (Setup Guide, 7.5). If a
limit is wrong, fix the limit there; do not accept your way round it.

---

## 7. Keys and the graph editor

### From a move to keys

Select a preset move and press **Convert to keys** (Inspector, graph editor or
right-click). The move becomes a keys clip that traces the same path, two keys
per axis, ready to shape. A keys clip can drive some axes and leave the others
to whatever came before it: **Axes this move drives** in the Inspector chooses.

### On the timeline

Open a camera's axis lanes with its triangle. Each key is a diamond.

| To | Do this |
| --- | --- |
| Move a key in time | Drag it |
| Select several | Drag a box round them |
| Delete keys | Select them and press **Delete** |
| Add a key at the playhead | Click a lane's name to select the lane, then **K**. With no lane selected, K keys every lane of the clip. |

### In the graph editor

The **Graph editor** tab shows the selected clip as one curve per axis. It
shares the timeline's zoom, scroll and playhead, and has the same scroll bar
along its bottom. The ruler on the left measures
the axis whose chip is outlined; click a curve to switch to it. Click an axis
chip to show or hide its curve.

| To | Do this |
| --- | --- |
| Change a key's time and value | Drag it. Shift keeps it to one direction. |
| Bend the curve | Drag a handle. The opposite handle stays in line; hold Alt to break it. |
| Choose how the curve leaves the selected keys | **Hold** (stay until the next key), **Linear**, **Bezier** |
| Make it start or land gently | **Easy Ease**, **Ease In**, **Ease Out** |
| Add a key without changing the shape | Double-click the curve |
| Fit the view | **F**, or **Frame** |
| See how fast each axis moves | **Speed** |

**Speed** is the view to check before you trust a hand-shaped move. The amber
line is the fastest that axis can go. Anything above it is filled amber and
named, and the clip turns amber on the timeline: that is a too-fast issue.

---

## 8. Record

Record captures a move you perform by hand, so a follow-shot or an instinctive
push-in can be kept and replayed exactly. It works in Rehearse only.

### Hand control

The **Rig controller** tab drives the selected camera:

- Drag the **pad** for pan and tilt.
- Drag the three **sliders** for zoom, focus and elevation.
- Everything springs back and the rig stops when you let go.
- Click the pad to use the keyboard instead: arrows for pan and tilt, `[` `]`
  for zoom, `,` `.` for elevation.
- **Top speed** scales all of it. Turn it down for fine work.

If the app stops sending (a dropped connection, a closed window) the rig stops
within a quarter of a second. Hand control is refused while armed.

To set the opening position for future songs, select the camera, frame the shot
with its hardware controller, and click **Store default** in **Rig controller**.
The server reads pan, tilt, zoom, and focus from that FR7 and height from its
AutoPod+ when configured. You can capture in Rehearse. If the rig cannot
report a current position, the camera keeps its previous default.

### Recording a pass

1. Open the camera's axis lanes and click the **red dot** on each lane Record
   should write. Pan, Tilt, Zoom and Elevation are on to start with; Focus is
   off. The same choice is in the
   Rig controller under **Record writes**.
2. Put the playhead a little before where the move should begin.
3. Press **Record**. The REC chip appears and the camera bar says that Record
   writes each camera you move.
4. Press **Space** to play, and perform the move. Any camera can be moved
   during the pass, and several in the same pass. On a camera you move, the
   axes you enabled follow your hands; everything else keeps playing the
   timeline.
5. **Stop**, **Cue** or **Record off** ends the pass.

Each camera that was moved gets one keys clip holding only the axes that were
moved, starting half a second before the first of them was. An axis nobody
touched writes nothing, and the frames before the first move are not filled in,
so what was on the timeline there is left alone. A camera that was not moved is
left as it was, and a pass in which nothing was moved writes nothing. One undo
removes the whole pass.

A camera put somewhere by hand before the pass starts its move from there. A
real head is moved from its own controller, and counts as moved when its
position changes: one that stands still is not recorded.

### Cleaning it up

The pass is reduced to as few keys as reproduce it closely: within 0.05 degrees
of pan and tilt, 0.2 % of zoom and focus and 1 mm of elevation. A stretch where
the rig stood still becomes two keys. The raw samples are kept, so with the
clip selected, **Fewer keys** and **More keys** in the Inspector fit them again
looser or tighter at any time. Fewer keys are easier to edit; more keys follow
the take closer. Then shape it in the graph editor like any other keys clip.

### Where the camera waits before a take

A take starts wherever the camera stood when the pass began, which is rarely
home. Before a camera's first recorded move the timeline therefore holds it
where that take starts, not at home, so there is no step when the move begins.
**Arm** does not drive every axis there at once: an axis the song never moves
is left where it stands, and an axis whose first move is still ahead stays put
until that move is near enough that a fade at **Arm moves at** arrives as the
take starts. Loading the song while the show is armed does the same. How fast
the fade travels is **Arm moves at** in that camera's rig settings: half its
top speed to start with, its top speed at 100 %.

A later take on the same camera that starts somewhere else is still a step,
and is reported as one.

### Smoothing a take

A real head reports where it is at uneven moments, not exactly on every
frame, so a take of a steady pan comes out as stand, jump, stand, jump. Played
back as recorded that is faster and sharper than the head can move, and the
song shows *too fast* and *starts or stops too sharply* on moves the camera
made without trouble.

**Smooth recorded moves** fixes them all at once. It is in the Inspector with
nothing selected, under **Recorded moves**; with a recorded clip selected,
**Smooth this camera's moves** does the same for that camera alone.

- Each recorded move that is over a speed or acceleration limit is smoothed
  until its rig can make it. Only the stretches over a limit are changed, by as
  little as brings them inside; the move keeps its frames and starts and ends
  where it did.
- A head that stops reporting while it moves leaves a take that stands still
  and then jumps to where the head had got to. That jump becomes the move the
  rig must have made: as quick as its limits allow, arriving on the frame the
  take does.
- A move that is still over a limit after that, because there is no room in the
  take to make it any slower, is accepted as programmed in your station's name,
  exactly as the button in chapter 6 does it.
- A recorded move that begins somewhere other than where the timeline has the
  camera, as a second take does when the head was moved in between, is a step
  no rig can make. It is given the move there instead, from its first frame,
  as quick as the rig's limits allow, and is back on the take after it.
- Range and keep-out problems are never accepted this way. They are left in
  the list, and the report counts them.
- The rig's limits are not changed, and neither are moves that were not
  recorded. A recorded move that was accepted while still over a limit is
  smoothed again from its take, and is no longer an accepted one once it is
  inside the limits.

When it is done the Inspector and the Log say how many moves were smoothed, how
far the furthest is from the take, how many were accepted and how many issues
are left. Smoothing works from the raw samples, so doing it again gives the
same moves, and a move that was trimmed or split is smoothed from its keys
instead. One undo puts the song back, and **Default** under **Recorded pass**
returns a single move to the take as it was recorded.

To record against the show's real clock, lock first (chapter 10): a move
recorded while locked replays against the same code.

---

## 9. Several cameras

A show holds up to twelve cameras. Each has its own lane, its own presets, its
own lens and its own place on the stage.

| To | Do this |
| --- | --- |
| Select a camera | Click its chip on the camera bar, its tile in Multiview, or the camera in the Stage tab. Or press `1` to `9`, then `0`, `-`, `=` for ten, eleven and twelve. |
| See all of them | **Multiview**: every camera's picture, name, state and focal length |
| See where they point | **Stage**: each camera's cone follows its pan, tilt and zoom as the song plays |
| Program one | Select it. The Presets tab now shows its presets; drag them onto its lane. |
| Move a clip to another camera | Drag it to that camera's lane |

A preset belongs to one camera, because the same framing from a different
position is a different pose. Wherever you drop it, the move lands on that
camera's lane, not the lane under the pointer. To give a move to another
camera afterwards, drag it to that camera's lane.

Ways to work that keep twelve lanes manageable:

- **Program one camera at a time**, start to finish, with Multiview open beside
  the timeline to see what the others are doing at the same moment.
- **Program by cue**: put markers on every cue first, then at each marker go
  along the cameras (`1`, `2`, `3` ...) dropping the preset each needs.
- **Stagger moves.** Two cameras should rarely be moving at the same instant
  if both might be on air. Because a move's right edge is when the camera
  settles, lining up right edges on markers shows at a glance who is steady
  when.
- **Keep lanes closed** except the one you are keying.

The camera bar's dots show each camera's state: simulated, connected, live,
not answering, held, or faulted. Hover a chip for the reason.

---

## 10. Audio, reference video and timecode

### Audio

Drop a WAV, AIFF or MP3 file on the window, or use **Import** in the **Media**
tab, then drag it from Media onto the **Audio** lane. Drag the clip to move it
and either edge to trim it; the waveform shows what is left. **Mute in
Program** in the Inspector keeps a clip out of the mix (for a guide track, or a
file that only carries timecode).

The server plays the audio, so it comes out of the server machine. The
waveform is the best guide for placing markers: play, listen, and press **M**
on each cue.

### Reference video

Video files (QuickTime or MP4) go on the **Reference video** lane. The lane
shows a filmstrip and the **Reference monitor** tab shows the frame under the
playhead, so camera moves can be programmed against a recording of a rehearsal.
The file's sound comes in with it. Video needs `ffmpeg` on the server machine.

### Files that carry their own timecode

On import the server looks for a start timecode in the file: a broadcast WAV's
`bext` chunk, or a QuickTime or MP4 timecode track. If there is none, select
the clip and type a start in the Inspector, or use **Read LTC** to decode it
from one channel of the file. That channel is then only read, never played,
and the rest of the file plays on both the left and the right output. The
media card then shows `starts 01:00:10:00`.

A stamped file dropped on the timeline lands on its own timecode wherever the
pointer is; the ghost says so. Hold **Alt** to place it by hand. If its code is
outside the song, a dialog offers to move the song's start to it. The Inspector
says whether a clip sits on its code and offers **Move to its timecode**.

### Lock

Free-run, the song plays on the sound card's clock. **Lock** makes it follow
timecode instead. The Lock button lists what it can follow:

| Source | What it is |
| --- | --- |
| LTC input | Timecode as audio on one channel of an input |
| MTC input | MIDI timecode |
| Art-Net timecode | From a lighting console or timecode generator on the network |
| A file on the timeline | Any audio or video clip in the song |

With an input locked, the main button reads **Play to code**. Press it and the
transport waits for code, takes four good frames to sync, and follows. The
chips tell you where it is:

| Chip | Meaning |
| --- | --- |
| WAITING FOR ... | Play to code is pressed, and no usable code is arriving |
| BEFORE SONG | Code is arriving, earlier than this song |
| AFTER SONG | Code is arriving, later than this song |
| OTHER SONG | Code is arriving inside a different song |
| LOCKED TO ... | Following |
| FREEWHEEL | Code dropped out; carrying on for a few frames before stopping to wait again |

If code jumps, the playhead jumps with it. Code that has not reached this song,
or has passed it, leaves Play pressed and the line under the buttons names that
code. Code at a different frame rate from the project is refused, and the bar
says which rate arrived.

**Follow timecode** (a tick box on the Set list tab) takes this from song to
song. Locked to an input with **Play to code** pressed, when the code leaves the
loaded song and is inside another, the server loads that one and plays it from
where the code is. It looks through the set list in order, or through every song
while the set list is empty, so give each song a start timecode of its own that
no other song's length runs into. Armed, the show stays armed and the rig
travels to the new song at the positioning speed while the song already plays,
so leave it a few seconds of code before the first move that matters. An armed
show only follows into a song that could be armed: one that is not committed, or
has an issue open, is not loaded, and the lock chip says which song and why.
Code that is in no song changes nothing.

**Lock to this file** (Inspector, or the file's entry in the Lock menu) ties
the transport to one clip: Play starts at the clip's first frame and stops at
its last, and Cue and the ruler work inside it. This is how to program against
a recording that carries the show's real timecode, long before the real
timecode source is on site. **Unlock** returns to free-run.

---

## 11. Save, Commit and revisions

| Command | What it does |
| --- | --- |
| **Save** (Cmd-S or Ctrl-S) | Saves the draft of every song with unsaved changes, not only the one that is open. The *unsaved* chip goes away. |
| **Revert** | Goes back to the last Save. Everything since is dropped, and this cannot be undone. |
| **Commit** | Freezes the saved draft as the next revision. Only a committed revision can be armed. |
| **Save As** | Makes a copy under a new name, with its own revisions, and opens it |
| **New**, **Load** | Start or open a song. Both need the transport stopped and disarmed. |

Your edits are never only in the app. Every change is sent to the server as
you make it and kept there, saved or not, so closing the app or losing the
connection loses nothing. *Unsaved* means "different from the last Save", which
is what Revert would return to.

The header shows the state:

| Chip | Meaning |
| --- | --- |
| *never committed* | No revision exists yet. Cannot be armed. |
| *revision 3* (green) | What is open is exactly revision 3. Can be armed. |
| *revision 3 + edits* | Revision 3 with changes since. Save and Commit to make revision 4. |
| *unsaved* (amber) | Changes since the last Save |

A rhythm that works: Save whenever something is right; Commit when a song is
ready to be shown to someone or run on the rig. Before a risky experiment, use
**Save As** to branch a copy.

**Load** lists every song with its start timecode and revision. From the list
a song can also be deleted, which removes its draft and every revision.

---

## 12. Going live

Everything so far has moved nothing. This chapter moves cameras. Clear the
area first, and know where E-STOP is.

### Arm

**Arm (moves rig to playhead)** is refused, with the reason on the button,
unless all of these hold:

- the transport is stopped and Record is off;
- the open song is a committed revision with no unsaved edits;
- every timeline issue is fixed, or (speed and acceleration only) accepted;
- every camera's devices are answering, and each AutoPod+ is on and calibrated.

When you press it, every camera's rig travels to its pose at the playhead, at
a share of its top speed (half, unless the rig settings say otherwise). The bar
reads **ARMING**, then **ARMED** when every axis of every camera has arrived.
While armed, the song and the project settings cannot be changed. Presets can
still be stored and deleted; a move keeps its own copy of the pose, so the
armed song does not change.

Put the playhead where you want the cameras to be *before* you arm. Arming at
the start of the song sends every camera to its opening position.

### Play Live

The main button now reads **Play Live** (or **Play to code** when locked).
Twenty times a second the server steers each axis toward the timeline and
compares where the rig says it is with where the timeline was at the moment of
that reading. The Program monitor's badge reads **LIVE**, and the **Rig
controller** tab shows how far each axis is from the timeline against its
tolerance.

| To | Press | The rig |
| --- | --- | --- |
| Pause the show | **Stop** | Holds where it is. Stays armed. |
| Go back to the start | **Cue** | Stops, and travels to the pose at the start. The bar reads ARMING until it is there. |
| Jump elsewhere while stopped | Click the ruler | Travels to the pose there, the same way |
| (Locked to timecode) the incoming code jumps | Nothing | Travels to the pose at the new code, the same way |
| Finish | **Disarm** | Returns to Rehearse. Nothing more is sent. |

### When a camera is held

A camera's rig is held where it is and its output is cut, with the reason in
red on the bar and in the Rig controller tab, when:

- an axis stays further from the timeline than its tolerance for longer than
  the tracking window (2 degrees of pan and tilt for half a second, unless the
  rig settings say otherwise);
- the next step would take an axis past its limit or break a keep-out rule;
- one of its devices stops answering;
- it does not reach the playhead pose while arming.

By default the other cameras carry on (**When a camera faults**, in the project
settings, can hold them all instead). The held camera stays held until you
**Disarm** and **Arm** again. Find out why first: the Log tab has the detail.

### E-Stop

**E-STOP** stops every camera and every pedestal, repeats the stop until they
report standing still, cuts output, and latches. It works from any station, in
any state, with or without the edit lease. While it is latched nothing can
move the rig. When it is safe, **Reset E-Stop**; you then arm again from
scratch.

E-STOP is a command the server sends over the network. It is not a hardware
emergency stop: if the server or the network is down it cannot reach the rig.
Know where the AutoPod+'s own stop control and the power are, and use them if
the screen does not answer.

Use it without hesitating. It costs a few seconds; a camera in the wrong place
costs more.

---

## 13. A show day

### The set list

The **Set list** tab holds the show's songs in running order. Press
**Arrange** to change it: **Add** puts a song at the end, and **Up**, **Down**
and **Remove** move it or take it out. Press **Arrange** again when the list is
right, and each row is only a song to load, with its start timecode and its
revision. Removing a song from the list does not delete the song, and deleting a
song takes it out of the list. The list belongs to the show, so it is changed
with the lease and disarmed, like the other show settings.

Click a song in the list to load it. **Next** and **Previous**, or Page Down
and Page Up, step through it. Loading needs the show stopped, not disarmed:
armed, the show stays armed, the chip reads **ARMING** while every camera
travels to the new song's opening position, and then **ARMED**. A lock to LTC,
MTC or Art-Net is kept across the change. Only a committed song with no open
issues is loaded into an armed show; the others say why on their row.

**Before doors**

1. Start the server on the show's data folder. Open the app. Take the lease.
2. Check the camera bar: every real camera connected, none *no answer*.
3. Load the first song. The header should read *revision N* in green, with no
   issue count. If it says *+ edits* or *unsaved*, decide now: Commit it or
   Revert it.
4. If the show is locked: **Lock**, choose the source, and check code arrives.
5. Clear the stage of people near the cameras. **Arm**. Watch every camera
   travel to its opening position. Wait for **ARMED**.

**During the show**

6. **Play Live** on the cue, or **Play to code** beforehand and let the
   timecode start the song.
7. Watch Multiview and the camera bar. A held camera shows in red; the others
   carry on.
8. Between songs: **Stop**, then **Next** on the Set list tab or Page Down, and
   wait for **ARMED**. Check every song's revision on the Set list tab before
   doors. With **Follow timecode** on and Play to code pressed, the timecode
   does this by itself.

**If anything looks wrong:** E-STOP first, questions afterwards.

**Afterwards**

9. **Disarm**. Back up the data folder. The Log tab, and `log/` in the data
   folder, hold exactly what was armed and what the rig did.

---

## 14. Programming from a panel

A SKAARHOJ PTZ Pro can do the work of chapters 3 to 13: start a song, make
presets, place and time moves, save and commit, rehearse, record a pass, arm
and play live. Connecting the panel, and programming a song on it from
nothing, is a manual of its own:
[PTZ Pro: setup and programming](ptz-pro.md). The base setup (cameras, lenses,
the rig, audio, importing media) stays in the app, and is in the
[Setup Guide](setup-guide.md#a-skaarhoj-panel-as-a-station).

The panel is a station like any other. It holds the edit lease under the name
*Panel*, the server applies the same rules to it, and when the server refuses
something the panel shows the server's reason, word for word.

> This has been tested against a stand-in panel and SKAARHOJ's published
> protocol, not yet on a real PTZ Pro. The first time, follow the check list
> in the Setup Guide with the rigs disarmed.

### What is where

| On the PTZ Pro | Does |
| --- | --- |
| **EXP, WB, Color, Image, U1, U2, U3** | The mode: Show, Transport, Camera, Presets, Timeline, Record, Media. The lit one is where you are. |
| **U4 and Image** | The eighth mode, **Program**. Image stays lit while you are in it. |
| **CAM 1 to CAM 7** | Soft keys. Each one's display says what it does now, in this mode. |
| **CAM 8** | The main transport button: the same next step as the app's main button (Rehearse, Play to code, Play Live, Stop, Reset E-Stop), in every mode. |
| **CAM 9** | **E-Stop**, in every mode. |
| **U4** | Shift. Hold it for the second set of soft keys, and for bigger steps on the knobs. |
| **P1 to P10** | Select camera 1 to 10. Cameras 11 and 12 are on the second page of Camera mode. In Program they mark moves instead. |
| **UP, DWN** | The previous and next page of a list (songs, cameras, presets, media). |
| **Focus knob** | The playhead: one frame a click. Turn it fast and it covers more; with Shift a click is one second. |
| **Menu 1 to Menu 4 knobs** | Four values for the mode, named on the display above each. |
| **Joystick** | Hand control of the selected camera, in Camera, Presets, Record and Program. Left and right pan, up and down tilt, twist zooms. |
| **Big display, top left** | The song (with `*` when unsaved), timecode and transport state, then in turn: mode and camera, who holds the lease, saved or committed, problems, lock. |
| **Big display, top right** | Messages: what was just done, why something was refused, what a hold will do, a question waiting for Confirm. A long message is shown a page at a time, in full. |

A dim key is available. A bright or coloured key also shows the state of
something: red for Record and for deleting, amber for something to attend to,
green for go, blue for what is selected. A dark key can do nothing right now:
press it and the display says why.

Choosing a mode or a camera never moves anything.

### Holds and questions

Some keys say **Hold** on their display. They move a real rig or take control
from someone, so a tap does nothing except say what a hold would do:

| Key | Hold for |
| --- | --- |
| **Arm** (Transport), and **Arm** when CAM 8 offers it | 1 second |
| **Play Live** and **Play to code** while armed, on CAM 8 or the Play key | 1 second |
| **Reset E-Stop**, on CAM 9 or CAM 8 | 1.5 seconds |
| **Take over** another station's lease | 1 second |

Keep the key down until the display changes. Let go early and nothing
happens. A hold is counted only if the panel is still answering when its time
is up: if the link to the panel drops during a hold, nothing is done.

Three things ask first: **Revert**, deleting a preset, and **Sync from rig** on
a real camera (it moves the camera to each preset). The display shows the
question, CAM 1 becomes **Confirm** and CAM 7 **Cancel**. No answer in ten
seconds counts as Cancel.

### E-Stop on the panel

Press **CAM 9** once, in any mode, with or without the lease, with Shift held
or a question on the display. The stop goes to the server on its own
connection and does not wait behind anything else the panel is doing. The key
then blinks **Hold: reset**.

To reset, the panel must hold the lease: hold CAM 9 (or CAM 8) for a second
and a half. The press that stopped the show never resets it, however long it
is held: let go and press again.

If the server cannot be reached the key reads **No server**, the display says
that E-Stop cannot reach the rig from the panel, and a press is still sent,
again and again for ten seconds, in case the server returns. As everywhere in
this program, E-Stop is a network command and not a hardware emergency stop
([E-Stop](#e-stop)).

### Show: the lease, songs, Save and Commit

| Key | Does |
| --- | --- |
| CAM 1 | **Take** the lease when it is free; **Release** it; or **Take over** from the station named on the key (hold) |
| CAM 2 | **New** song: *Song N*, starting on the next ten minutes of timecode after the latest song, and loaded |
| CAM 3 | **Browse**: the song list on CAM 1 to CAM 6, the loaded one lit, UP and DWN for more, CAM 7 **Back**. Press a song to load it. |
| CAM 4 | **Save** (amber when there are unsaved changes) |
| CAM 5 | **Commit** (amber when the saved song is not yet a revision) |
| CAM 6 | **Save copy**: a copy of the song under a new name |
| CAM 7 | **Revert** to the last Save (asks first) |

Names are given by number. Rename songs, presets and markers in the app.

### Camera: driving by hand and storing presets

CAM 1 to CAM 6 are the cameras, six to a page, each with its state; press one
to select it, or use P1 to P10 from any mode. The joystick drives the selected
camera. The Menu 1 and Menu 2 knobs drive **Focus** and **Elevation**, a short
nudge for each click. Menu 3 is **Top speed**: 5 % to 100 % of each axis's top
speed at full deflection; turn it down for fine work. CAM 7 **Store** saves
where the camera is as *Preset N*.

The rig stops when you let go. It also stops if the panel, its cable or the
server is lost: see [what happens when things drop](setup-guide.md#a-skaarhoj-panel-as-a-station).
After that, and whenever the joystick is already pushed over at the moment it
is given the rig (entering the mode, changing camera), the display says *Let
the joystick come to rest, then drive again*: let go, then push.

As in the app, hand control is refused while armed, and while playing unless
Record is on. It drives the simulator only. For a real FR7 the server refuses
it and the panel shows why; put the camera where you want it with its own
controller, then press **Store**.

In any mode that does not drive by hand, pushing the joystick does nothing
and the display says *Press Camera to drive by hand*.

### Presets: placing moves

| Key | Does | With Shift |
| --- | --- | --- |
| CAM 1 | **Store** the camera's pose as a new preset | **Sync from rig** (asks first on a real camera) |
| CAM 2 to CAM 6 | **Place** a move to that preset at the playhead, on the selected camera's lane. It takes its auto time. | **Delete** that preset (asks first) |
| CAM 7 | **Undo** | **Redo** |

Put the playhead where the move should begin (Focus knob), then press the
preset. UP and DWN page through more than five presets. The joystick and the
Camera knobs work here too.

### Program: marking moves against the song

Program is for getting a song down quickly when the framings are set on
another controller: the FR7's own remote, a PTZ controller, the AutoPod+'s
panel. The PTZ Pro runs the timeline like a tape, and each camera's number
says *this is where it stands* and then *this is when it must be there*.
Enter it with **U4 and Image**.

| Key | Tap | Hold |
| --- | --- | --- |
| CAM 1 **Back** | Five seconds back | Winds back: 8 times speed, 24 times after three seconds |
| CAM 2 **Play** or **Pause** | Plays from the playhead; pauses and leaves the playhead where it is | |
| CAM 3 **Forward** | Five seconds forward | Winds forward |
| CAM 4 **To start** | The playhead to the start of the song | |
| CAM 5 **Cancel** | Takes back the newest mark | |
| CAM 6 **Save** | Saves every song with unsaved changes (amber when the loaded song has some) | |
| CAM 7 **Undo** | Undo; with U4, **Redo** | |

With **U4** held, a tap on Back or Forward is **one frame** and a hold steps
**frame by frame**, about ten a second. The keys' displays say *5 s* or
*1 frame*. A key let go within a third of a second is a tap. The Focus knob
moves the playhead here too, as everywhere.

Winding stops when you let go, at either end of the song, and if the panel or
its cable is lost. It is refused while the show is locked to timecode
(*Unlock to cue*).

**Marking a move** takes two presses of the camera's number, **P1 to P10**:

1. Put the playhead where the camera should **leave** for its next framing,
   and pause.
2. Set the framing on the camera's own controller: pan, tilt, zoom, focus and
   the AutoPod+'s height.
3. Press the camera's number. The server reads where the rig stands and
   stores it as *Preset N*, or uses a preset that camera already has when the
   pose is within range of one (below). The key **flashes amber** and the
   display says which preset it is and the time of the mark.
4. Play, wind or step to where the camera must **have arrived**, and press
   the same number again. The move is placed on that camera's lane: it leaves
   at the first press and arrives at the second, eased in and out. The key
   stops flashing and the display gives both times and the length.

The number may be pressed with U4 still held from stepping by frames. Only in
a show with eleven or twelve cameras does U4 change the key: P1 and P2 are
then cameras 11 and 12, as everywhere.

Several cameras can be marked at once, each finished at its own time: mark
three while paused, play, and press each number on its beat. A second press
at the mark's own time, or before it, places the move at the playhead with
its auto time, as the Presets mode does.

If the two presses are closer together than the rig can travel, the move is
still placed and the display says **PROBLEM** with the reason; mark it longer
(Undo, then again), or fix it in Timeline. If the move cannot be placed at
all, because another clip of that camera is in the way, it is refused with
the reason and the key goes on flashing: go somewhere else and press again,
or **Cancel**.

**Cancel** (CAM 5) takes back the newest mark. A preset that the mark stored
is removed with it; a preset that was already there is kept. Marks are also
dropped when another song is loaded and when the panel loses the lease; the
presets they stored stay.

**A preset within range.** So that twenty presses on nearly the same framing
do not make twenty presets, the first press looks for a preset of that camera
within a small range on every axis and uses the nearest. The ranges, and
whether to look at all, are in Show settings on the **Programming** page: out
of the box one degree of pan and tilt, 2 % of zoom, 5 % of focus and 10 mm of
height. When a preset is used again the display says so (*Cam 2 is within
range of Wide: using it*), and the move goes to the stored preset, not to the
slightly different place the camera stood.

Program works only while the show is **disarmed**. Armed, moving the playhead
moves the rig, so every Program key is dark and says *Disarm to program*.
Disarmed, playing and winding move no real rig: the camera stays where its
controller left it, which is what the next press reads. A real rig must be
connected (Show settings, **Rig connection**) for its pose to be read; if it
is not answering, the press is refused with *The rig's pose is not known*.

On the simulator there is no other controller: set the framing with the
PTZ Pro's joystick and the Menu 1 to Menu 3 knobs (Focus, Elevation, Top
speed), which work in Program as they do in Camera. The simulator shows the
camera where the song has it whenever the playhead moves, so set the framing
after the playhead is where you want the mark.

> Reading a real rig while another controller moves it has been tested
> against the FR7 and AutoPod+ protocol stand-ins, not yet with two real
> controllers on one real FR7.

### Timeline: shaping what is there

| Key | Does | With Shift |
| --- | --- | --- |
| CAM 1, CAM 2 | **Prev** and **Next** clip on the selected camera's lane. The playhead goes to the clip. | Playhead **To start**, **To end** |
| CAM 3 | **Ease**: steps through the eases of a preset move | **To keys**: turn the move into keys |
| CAM 4 | **Auto time**: give the move its auto time again (amber when that would clear a problem) | **Accept as is**, or **Withdraw accept**, for a speed or acceleration problem |
| CAM 5 | **Duplicate** the clip | Marker **Add** at the playhead |
| CAM 6 | **Delete** the clip | Marker **Delete** at the playhead |
| CAM 7 | **Undo** | **Redo** |

| Knob | Does |
| --- | --- |
| Menu 1 **Move clip** | Slides the selected clip earlier or later |
| Menu 2 **Length** | Makes it longer or shorter |
| Menu 3 **Clip edge** | Jumps the playhead to the next or previous clip edge |
| Menu 4 **Marker** | Jumps to the next or previous marker; push the knob to add one |

A move that would break a rule is refused with the reason, as in the app, and
problems are counted on the status display. Undo is one key away in Presets,
Program, Timeline, Record and Media, and goes back through the changes made from the
panel since the song was loaded, up to two hundred of them.

### Transport

| Key | Does |
| --- | --- |
| CAM 1 | **Rehearse**, **Play to code** or **Play Live** (hold while armed) |
| CAM 2 | **Stop** |
| CAM 3 | **Cue** |
| CAM 4 | **Arm** (hold). Reads **Arming** while the rig travels, then **Armed**. |
| CAM 5 | **Disarm** |
| CAM 6 | **Lock to** the source named on the key, chosen with the Menu 1 knob; **Unlock** when locked |
| CAM 7 | **Record** on and off |

CAM 8 offers the next step in every mode, so a rehearsal is usually just CAM 8
to play and CAM 8 to stop.

### Record: performing a move

1. Record mode. CAM 1 to CAM 5 are the axes Record writes (Pan, Tilt, Zoom,
   Focus, Elevation): red is on. Choose them while stopped.
2. CAM 6 **Record** on. The display names the camera and the axes.
3. CAM 8 to play, and drive the camera with the joystick.
4. CAM 8 to stop. The pass lands on the timeline as a keys clip, selected.
   Stopping turns Record off. **Undo** (CAM 7) removes the pass.
5. With Shift, CAM 1 **Fewer keys** and CAM 2 **More keys** refit the selected
   pass; the display says how many keys it had and has now.

### Media

Files are imported in the app. On the panel, CAM 1 to CAM 4 **Place** an
imported file at the playhead (UP and DWN for more files), CAM 5 selects a
media clip on the timeline, and CAM 6 mutes or unmutes it. With Shift, CAM 5
takes the selected clip off the timeline and CAM 6 locks the song to that
file's own timecode, or unlocks.

### A show from the panel

1. Show: **Take** the lease, **New**.
2. Camera: P1, drive to the first framing, **Store**. Repeat for each framing
   and each camera.
3. Presets: Focus knob to where a move starts, press the preset. Repeat.
4. Timeline: **Next** through the moves; **Ease**, **Length** and **Move
   clip** to taste; fix or accept anything the status display counts as a
   problem.
5. CAM 8 to rehearse, CAM 8 to stop. Shift and **To start** to go back.
6. Show: **Save**, **Commit**.
7. Clear the area. Transport: hold **Arm** and wait for **Armed**. Hold
   **Play Live** on the cue. **Stop**, then **Disarm**.

If anything looks wrong: CAM 9.

With the framings set on another controller, steps 2 and 3 become Program
mode: U4 and Image, then for each move pause, set the camera, press its
number, go to where it must have arrived, and press its number again.

### Not on the panel

Typing names; the graph editor and editing single keys; project, camera, lens,
rig, limit and audio settings; the stage view; importing media; revisions
other than the latest; the run log. Do these in the app, which can be open on
another station at the same time: the panel keeps the lease, and the app
watches until it takes over.

---

## 15. Reference

### Keyboard

On a Mac, Cmd; elsewhere, Ctrl. Save is both **Cmd-S** and **Ctrl-S**, on every machine.

| Key | Does |
| --- | --- |
| Space | Play or pause at the current frame, including in Live edit. After an armed seek, wait for **ARMED** before pressing Space to play. |
| Home | Cue to the start |
| Page Down, Page Up | Load the next song in the set list, or the one before |
| End | Playhead to the end of the song |
| Left, Right | Step one frame. With Shift, ten. |
| `1` to `9`, `0`, `-`, `=` | Select camera 1 to 12 |
| M | Add a marker at the playhead |
| K | Add a key at the playhead on the selected lanes |
| Delete, Backspace | Delete the selection |
| Cmd-D | Duplicate the selection after itself |
| Cmd-C, Cmd-V | Copy the timeline selection, paste it at the playhead |
| Cmd-Z, Shift-Cmd-Z | Undo, redo |
| Cmd-S, Ctrl-S | Save |
| `\` | Fit the song to the window |
| F | Graph editor: fit the view |
| Escape | Cancel a drag, close a dialog, or close Show settings |
| Shift while dragging | Snapping off. In the graph editor, keep to one direction. |
| Alt while dragging | Copy a clip. In the graph editor, break a handle. Place a stamped file by hand. |
| Arrows, `[` `]`, `,` `.` | With the Rig controller pad clicked: pan and tilt, zoom, elevation |

Single keys do nothing while you are typing in a field. There is deliberately
no key for E-Stop, Reset E-Stop, Arm or Disarm: Space runs the transport, and
those four take a click on their own button.

### State chips on the transport bar

| Chip | Meaning |
| --- | --- |
| REHEARSE | Nothing is sent to the rig |
| ARMING | The rig is moving to the pose at the playhead |
| ARMED | Play moves the rig |
| FREE RUN | The server's own clock drives the playhead |
| WAITING FOR, LOCKED TO, FREEWHEEL | Following timecode (chapter 10) |
| REC | Play writes the rig's movement onto the record axes |
| E-STOP LATCHED | Nothing can move until Reset E-Stop |

### Why a button is disabled

Hover any disabled button and it says why. The reasons you will meet most:

| Reason | What to do |
| --- | --- |
| Take the edit lease to control the show | **Take the lease** in the header |
| Create or load a song first | **New** or **Load** |
| Stop and disarm to change songs | **Stop**, **Disarm**. A new song is started in Rehearse. |
| Stop to change songs | **Stop**. Loading another song does not need Disarm. |
| ... is not committed. Commit it, or disarm, to load it | An armed show loads only what could be armed |
| Commit the song first | **Save**, then **Commit** |
| Fix or accept N timeline issues first | Click the issue count in the header |
| Stop before arming, Stop before locking | **Stop** |
| Turn Record off before arming | **Record off** |
| Disarm to record | Record works in Rehearse only |
| Disarm to drive the rig by hand | Hand control is refused while armed |
| The rig is still moving to the playhead. Wait, or Disarm | Wait for ARMED |
| Reset E-Stop to continue | **Reset E-Stop**, when it is safe |

### Glossary

| Word | Meaning |
| --- | --- |
| Accept as programmed | Allow Arm although a move is faster than the rig; the rig will lag there |
| Arm | Move every rig to the playhead pose and make Play move hardware |
| Auto time | A move's length worked out from distance and the rig's limits |
| Axis | Pan, Tilt, Zoom, Focus or Elevation |
| Commit | Freeze the saved draft as a numbered revision |
| Cue | Return the playhead to the start |
| Custom time | A move's length set by hand |
| Edit lease | The right to change the show, held by one station at a time |
| Freewheel | Carrying on briefly when timecode drops out |
| Held | A camera stopped where it was after a fault, while the others carry on |
| Issue | A place where the timeline asks for something the rig cannot or must not do |
| Keep-out rule | A forbidden combination of two axes |
| Keys clip | A move shaped by keys on per-axis curves |
| Lock | Follow timecode instead of the server's own clock |
| Pass | One recorded take |
| Preset | A stored pose for one camera |
| Preset move | A clip that takes a camera to a preset |
| Rehearse | Play audio and monitors without sending anything to the rig |
| Revision | A committed, frozen state of a song. Only revisions can be armed. |
| Stamped file | A media file that carries its own start timecode |
