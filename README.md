# Duet

Free peer-to-peer jamming for two people, with a pedalboard. Audio goes straight
from one computer to the other over UDP: no server, no account, no subscription.

## Install

**Windows:** run `Duet.exe`. SmartScreen will warn because it isn't signed:
*More info → Run anyway*. Allow it through the firewall when asked.

**Mac:** unzip `Duet-mac-apple-silicon.zip` (M1/M2/M3/M4 Macs) or
`Duet-mac-intel.zip` (older Intel Macs; Apple menu → About This Mac tells you
which) and drag `Duet.app` into Applications. The app isn't signed by Apple, so
the first launch needs one extra step:

- Open it once. macOS says it can't be opened → click *Done*.
- System Settings → Privacy & Security → scroll down → **Open Anyway** next to Duet.
- Or, in Terminal: `xattr -cr /Applications/Duet.app`

Then allow **Microphone** and **incoming network connections** when asked. If you
denied the microphone: System Settings → Privacy & Security → Microphone → Duet on.

## Using it

Open Duet and sound is on: it picks your audio interface (on Windows the
`ASIO` one) by itself. The panel on the left (button top-left to show/hide it)
has two tabs:

1. **Connect:** *Copy* your code and send it to your partner; paste theirs into
   the box and it connects right away. **Both** of you paste each other's code.
   That's what gets through your routers. You only do this once: your code stays
   the same between sessions, so next time Duet reconnects by itself. Once
   you're connected the panel slides away.
2. **Sound:** change interface, headphones, sample rate or buffer; it restarts
   by itself. **Same buffer and sample rate on both sides** (128 / 48k is a good
   start).
3. **Mixer:** one strip per input you play into, up to four (*+ Add input*, e.g.
   guitar on In 1 and vocals on In 2). Click a strip's name to rename it; your
   partner sees that name on their side. Per strip:
   - **SEND**: your partner gets this channel (as its own strip on their side)
   - **MON**: you hear it yourself. Turn off your interface's direct monitoring
     then, or you'll hear yourself twice.
   - **M / S**: mute / solo; the fader and pan knob set your own mix
   
   Your partner's channels appear after the divider, one strip each, and you
   mix them however you like. **Safety buffer** (in the mixer's title bar): on
   **Auto** it finds the smallest buffer that doesn't crackle by itself: up a
   step on a dropout, down a step after 10 s without trouble (on a quiet moment,
   so you don't hear it). − / + set it by hand instead; each step is one block
   (about 2.7 ms at 128 / 48k).
4. **Effects:** a strip's effects are slots on the strip, like in Logic or FL.
   Click one and just that effect opens in a small window of its own (drag it by
   its title; several can be open; *Esc* closes the one in front). The dot on a
   slot switches it on/off; right-click a slot to move it up or down or remove
   it; **+** adds an effect, a VST3 plugin or a preset (which replaces the
   strip's effects). Drag knobs up/down (Shift = fine, double-click = reset,
   scroll wheel works). Effects on your strips are heard by
   your partner; effects on their strips only change what you hear (e.g. some
   reverb on their vocals). The effects: **EQ** (low and high cut, low and high
   shelf, a mid band, with its curve drawn), **Compressor** (attack, release, and
   a sidechain on song tracks), **Limiter**, Overdrive, Chorus, Tremolo (*Stereo*
   turns it into an auto-pan), Delay (*Ping-pong*), Reverb (*Width*), NAM amps
   and VST3 plugins. Everything is stereo inside Duet; what goes to your partner
   live is mono, to keep the line light.

5. **NAM amps:** *+ Add effect → NAM Amp* adds a Neural Amp Modeler slot. Load a
   capture with **TONE3000** (pick a tone on tone3000.com in your browser; Duet
   downloads it), **Open…** (any `.nam` file) or the **Library** (everything you
   loaded before). *Input* drives the amp harder, *Output* sets the level; models
   are level-matched so switching doesn't jump in volume. If the pedal says the
   model was captured at another sample rate, click *Switch*.
   - TONE3000 needs a free **publishable key** once: on tone3000.com go to
     Settings → API Keys, create one, and paste the `t3k_pub_…` key when Duet asks.
   - NAM is the heaviest thing in the rack: a *standard* capture uses about a
     fifth of each audio block on a recent PC. Watch **DSP** in the bottom bar; if
     it gets near 70%, use the *lite* / *feather* version of a capture.

6. **VST3 plugins:** *+ Add effect → VST3 plugin…* lists the effects in your
   VST3 folder (search by name, maker or type). *Open window* shows the plugin's
   own controls; *Mix* blends dry/wet. Plugins run inside Duet with no extra
   delay. (Instruments are under *Instruments*, below.) The first scan takes
   about half a minute; after that it's instant. A plugin that hangs or shows a
   license dialog while being scanned is skipped, not fatal.
7. **Metronome** (*Jam* in the top bar, for playing live; its tempo is its own,
   apart from the song's): on/off, tempo (type it, −/+, or *Tap*) and
   beats per bar are shared with your partner; the click volume is just yours
   and the click is never sent. Whoever starts it or changes the tempo leads;
   the other one's click is lined up with the leader's playing *as it arrives*,
   so you play along to what you hear. Tempo-synced plugins follow it too.

## The workspace

Like FL Studio, everything is a window you arrange yourself: the **Playlist**
(F5), the **Channel rack** (F6), the **Piano roll** (F7) and the **Mixer** (F9),
with each effect in a small window of its own. The buttons in the top bar open a
window, bring it to the front, or (already in front) close it.

- **Move** a window by its title bar; **size** it from any edge or corner. Edges
  stick to the workspace's edges and to the other windows.
- **Snap:** drag a window against an edge of the workspace and let go: it fills
  that half (a corner: that quarter; the top: all of it). Double-click the title
  to fill the workspace, and again to go back.
- **Layouts:** the button next to the window buttons has ready-made layouts
  (*Compose*, *Arrange*, *Mix*, *Mixer on the right*, *Piano roll big*) and
  *Save this layout…* for your own. The layout you have is kept for next time,
  and it scales along when the app's window changes size.

## Recording a song together

The **Playlist** is a timeline you both work in at
the same time, like a shared document: the song is open on both computers and
every change (a new track, a moved clip, the tempo) shows up on the other side
right away. The green dot on the song name means you're in the same song.

1. **Tracks:** *+ Track* asks what goes on it. **Audio · line in**: pick **who
   plays on it and from which input** (*You · Guitar* or *Partner · Vocals*);
   it's armed right away. **Software instrument**: a drum kit, the synth or a VST,
   with a pattern for it already in the song at the playhead, and the sequencer
   open to write in. Orange tracks are yours, teal ones your partner's.
2. **Arm** the tracks you want to record (the red dot). Arm yours and theirs at
   once to record both of you in one go.
3. **Record** (the red button, or *R*). With *1 bar count-in* on you get a bar
   of clicks first. Either of you can press it; it starts on both sides.
4. **Stop** (or *space*). The takes **pop up as clips** on their tracks, for both
   of you. Your partner's take shows as "Receiving…" for a few seconds while the
   audio comes over, then turns into a waveform.

Each of you records **on your own computer**, in full quality and against your
own playback of the song, corrected for your own sound card. So the takes land
on the beat **however far apart you are**: the 30–50 ms between you only
matters for what you hear of each other live, not for what ends up in the song.

**Editing clips** (right-click a clip for the same things in a menu):

| | |
|---|---|
| Select | click; shift-click for more; drag a box on empty space; ctrl+A for all |
| Move | drag it, also to another track (snaps to the grid; hold *Alt* to move freely) |
| Copy while moving | hold *Ctrl* (*Cmd* on a Mac) while you drag |
| Trim | drag its left or right edge |
| Fade in / out | drag the little square in its top left / top right corner |
| Volume | right-click → *Volume* |
| Split | *S* splits at the playhead (right-click: split where you clicked) |
| Duplicate | *Ctrl+D* puts a copy right after it |
| Copy / cut / paste | *Ctrl+C* / *Ctrl+X* / *Ctrl+V* (pastes at the playhead) |
| Delete | *Delete* |
| Undo / redo | *Ctrl+Z* / *Ctrl+Y*: your own changes only, never your partner's |
| Crossfade | select two clips that meet on a track, *X*: they overlap a little, one fading out as the other fades in |
| In / out points | *I* and *O* at the playhead (the loop); *Ctrl+U* around the selected clips |
| Trim / fade to the playhead | *Alt+[* / *Alt+]* trim the clip's start / end to the playhead; *Alt+I* / *Alt+O* fade in up to it / out from it |
| Nudge | *Alt+←* / *Alt+→* one grid step |
| Edit the clip | double-click it (or *E*): the clip editor, below |

**Shortcuts are yours to set:** the song menu → *Keyboard shortcuts…* (or
*Ctrl+/*). Start from Duet's, or from a set in the style of FL Studio, Logic,
Pro Tools (*A*/*S* trim, *D*/*G* fade, *F* crossfade, *B* split) or Premiere/Avid
(*I*/*O*, *Ctrl+K* split), then click any shortcut and press the key you want.
A key does one thing: giving it to something else takes it away from what had it.
The crossfade length is set there too. They're kept on this computer (your
partner has their own).

**The clip editor** (double-click an audio clip). Everything in it changes the
clip, never the file, and is part of the song: your partner hears the same,
and *Undo* takes it back.

- **The waveform** shows the whole file with the clip's stretch of it lit up.
  Drag its edges to trim, drag inside to slide the sound under the clip (the
  clip stays where it is in the song). Scroll to zoom, shift-scroll to move.
  ▶ plays the song from the clip.
- **Clip:** volume and *Normalize* (as loud as it goes without clipping), fade
  in and out, *Backwards*, and *Reset everything*.
- **Pitch & speed:** transpose (semitones, and cents for fine tuning) and
  speed (25% to 400%), in one of two modes:
  - *Keep pitch*: speed and pitch apart, like Logic's Flex or FL's stretch
    modes. The sound is stretched in the background (Signalsmith Stretch, a
    moment for a few minutes of audio); the clip says *Stretching…* and is
    silent until it's ready. An export waits for it.
  - *Tape*: faster is higher, like a record. Instant, no artifacts.
  *Detect* finds the sound's tempo; *Fit to the song* then plays it at the
  song's tempo. *Make it last 1/2/4/8/16 bars* stretches it to exactly that.
- **Chop:** cuts where every hit starts (*Find the hits*, with a sensitivity
  slider), or on the grid (bar, beat, 1/8, 1/16), or click your own; drag a cut
  to move it, right-click it to take it away. *Chop into clips* cuts the clip
  at every mark: each piece is its own clip, to move, copy or delete.
- **Stems:** split the clip's sound into drums, bass, vocals and the rest (or
  just vocals + instrumental), each on a new track under this one, with a clip
  in the same place (same speed, pitch and fades). The clip itself is muted
  (right-click → *Unmute clip* to hear it again). Also right-click a clip →
  *Split into stems…*. It's Demucs, Meta's open-source stem splitter, working
  on this computer: nothing is sent anywhere. Its model (174 MB) is downloaded
  once, from Duet's releases on GitHub, the first time you use it. A 4-minute
  song takes somewhere between half a minute and a few minutes, depending on
  the computer; it runs in the background (the clip shows how far it is, and
  *Stop* stops it). Your partner gets the stems as files, like a recording,
  and doesn't need the model. (On Apple Silicon Macs, M1 and newer: the Intel Mac build doesn't have it
  yet, as ONNX Runtime, which runs the model, has no ready-made Intel version.)

Any clip can be muted: right-click → *Mute clip* (or *Alt+M*). It stays in
place, greyed out, and is silent for both of you.

Click in the ruler to move the playhead, drag in the ruler to mark a loop.
*Space* plays and stops, *R* records, *Ctrl + scroll* zooms. *Click* gives you
a metronome while the song plays (just for you).

**Export** (the song menu top left, or *Ctrl+E*) saves the song as a 24-bit WAV: just the
mix, or the mix plus every track on its own (stems, e.g. to mix it further in
another program), the whole song or just the loop. It renders faster than real
time and the sound keeps running meanwhile. What you hear is what you get: the
faders, pans, effects, mute and solo as you have them.

Each track also has a strip in the **Mixer** (under *Song*) with volume, pan
and its own effects, NAM amps and VST3 plugins included; *FX* on a track header
takes you there. Volume, pan and effects are part of the song (you both hear
them); mute and solo are just for you. A VST on a song track needs that plugin
on both computers (without it, that slot lets the sound through and says so).

## Mixing the song

Everything below is part of the song, like the tracks themselves: you both
hear it, either of you can change it, and the export sounds exactly like it.

- **Buses:** *+ Bus* in the mixer adds a reverb bus, a delay bus or an empty one.
  A bus is a mixer track that other tracks feed: one shared room for every
  track that sends to it, or a group (all drums through one compressor, one
  fader for all of them).
- **Routing, like FL Studio:** click a channel in the mixer, then the **arrow**
  at the foot of a bus: the channel **sends** some of its sound there, after its
  fader, on top of its own (a dashed cable shows it, and a small knob above the
  arrow sets how much). Click the arrow again to stop. Right-click it, *Route
  here only*, to send the whole channel to the bus instead of the master (an
  orange cable; the channel's **→ Master** button picks its output too). Up to
  four sends per channel. A bus can go to another bus, as long as it doesn't
  loop back. Select a bus to see everything that feeds it.
- **Live channels** (yours and your partner's) can send to a bus too: one shared
  reverb while you jam. That part is just for your own ears, kept with the song
  on your computer; your partner hears you as you send it.
- **Sidechain:** a compressor on a song track or the master can listen to
  another track instead of its own sound (*Sidechain:* under its knobs): the
  kick pushes the bass down.
- **Master FX:** the *Master* strip's effects shape the whole song (an EQ and a
  limiter at the end make it fuller and louder). They're on what you hear live
  too.
- **Delay compensation:** plugins that delay the sound (the limiter's
  look-ahead, many VSTs) don't put a track behind the beat: the song is read
  that much earlier, also through buses with slow plugins on them. The rack's
  title says how much (*1.5 ms delay, compensated*). Recording allows for it
  too. What you play live can't be read early: a slow plugin on your own input
  delays it by its latency, as in any DAW.

Songs are saved by themselves. The song name opens the song menu: rename it,
start a new one, or open an older one (your partner switches along). If you
both had a different song open when you connected, Duet asks which one to use;
the other one stays saved.

## Automation

A knob that moves by itself while the song plays: a filter opening up, a
reverb growing into the chorus, a fade-out. Each automation lane is a row in
the playlist, right under the track it moves.

- **Make one:** right-click any knob or fader of a song track or the master
  (in the mixer, an effect's window, an instrument's window) and choose
  *Automate*. Or right-click a track's header, or empty space in the playlist,
  for everything on that track (volume, pan, every effect knob, the
  instrument's knobs). A new lane starts flat, at the value the knob has now.
- **Draw it:** click to add a point, drag a point to move it (it snaps to the
  grid; hold Alt to place it freely), Alt-click or right-click to take one away.
  Points are joined by straight lines; before the first and after the last the
  value stays where it is. Two points on the same beat make a jump.
- **What it moves** can be changed with the menu in the lane's header (another
  knob on the same track). If the effect it moves is taken out of the rack, the
  lane goes grey and says so; put the effect back and it works again (an effect
  moved to another place in the rack is followed).
- While a lane is there it decides: the knob's own setting comes back when the
  lane is deleted. It works stopped too (the sound is the one at the
  playhead), on your partner's side (it's part of the song), and in an export.

## Sequencer: patterns, channel rack, piano roll

The **Sequencer** tab works like FL Studio's channel rack and piano roll, in
the shared song (your partner sees every note you place, and you theirs).

- **Channels** are the song's instruments: *+ Track → Software instrument* in the
  playlist, or *+ Channel* here (it joins the open pattern). A channel is also a
  track in the playlist and a strip in the mixer (*Sound* opens its knobs, pads
  and effects there).
- **Patterns** hold notes for any number of channels. Pick one at the top (or
  *+ New pattern*), set its length (1–8 bars); *⋯* renames, copies, clears or
  deletes it.
- **Steps:** every channel has a row of 16th-note steps per bar; a drum kit has a
  row per pad (*All pads* shows all 16). Click a step to switch it on or off.
- **Piano roll** (below; drag the bar above it for more or less room) for the
  selected channel: click to draw a note, drag to move it (up/down changes the
  pitch), drag its right end to make it longer. Notes snap to the grid (*Snap*,
  or hold *Alt*). The bars underneath are the velocities: drag them up or down
  (or *Alt + scroll* on a note). Click a key on the left to hear it. The other
  channels' notes show faintly behind, to write against.

  | | |
  |---|---|
  | Erase | right-click, or hold the right button and drag over notes |
  | Select | *Shift* (or *Ctrl*) + drag a box on empty space; *Shift*-click a note; *Ctrl+A* |
  | Copy while moving | *Ctrl* + drag a note |
  | Copy / cut / paste | *Ctrl+C* / *Ctrl+X* / *Ctrl+V* (pastes at the mouse) |
  | Duplicate | *Ctrl+B* puts a copy right after the selection |
  | Quantize | *Ctrl+Q*, with a bar to pick the grid (triplets too), the strength and whether the ends line up |
  | Humanize | *Ctrl+H* loosens the timing and velocity a little |
  | Legato | *Ctrl+L*: every note lasts until the next one |
  | Move | arrows: a semitone (*Shift* / *Ctrl*: an octave), a grid step left/right |
  | Delete, undo | *Delete*, *Ctrl+Z* / *Ctrl+Y* |

  *Ctrl+Q*, *H* and *L* work on the selection, or on all of the channel's notes.
- **▶** plays the pattern on its own, looped (*space*), while you work on it.
- **Into the song:** *+ Into the song* puts the pattern in the playlist at the
  playhead, or double-click an empty spot in the playlist. A pattern block plays
  the pattern's notes on their channels; drag its right end to make it longer and
  the pattern repeats. Blocks move, trim, split, copy and delete like clips;
  double-click one to edit the pattern.
- **Recording notes:** arm a channel (its red dot in the playlist) and record:
  what you play on the MIDI keyboard or the computer keyboard becomes a new
  pattern, placed where you started.

Patterns play on both computers from the same notes, so they sound the same and
in full quality on both sides (a VST instrument needs to be installed on both).

## Working side by side

You see where your partner is working, in their colour, as if you were looking
over their shoulder:

- **Their pointer** (an arrow with *Partner*) in the playlist and in the piano
  roll, when they're on the same notes as you. When it's out of view, a
  *Partner ↓* tag on the edge points to where it is.
- **What they've selected:** a dashed outline around their clips and notes.
- **Where they are:** the teal pill at the top says it (*Partner · Sequencer ·
  Beat · Drums*, *Partner · playing Bass synth*…); click it to go there. In the
  sequencer, the channel they're on says *Partner*, and the bar at the top shows
  which pattern they have open (click it to go there too). While stopped, a teal
  mark on the ruler shows where their playhead stands.
- **Follow** (in the pill) keeps your view on theirs: the same tab, pattern,
  channel and spot in the song, as they move. It stops as soon as you click or
  scroll yourself.

**Playing a channel live:** whatever you play on a channel (MIDI keyboard,
computer keyboard, a piano roll key or a step you click) your partner hears
right away, on their side. Only the notes go over the line, not the sound: their
copy of the instrument plays them, in full quality and with almost no data. On
their side the channel glows and the keys light up in the piano roll. When you
record, your notes end up in the song through you, so nothing is recorded twice.
(A VST instrument needs to be installed on both computers.)

## Browser: your sounds

The **Browser** tab on the left (next to *Connect* and *Sound*) is your sample
library, like FL Studio's:

- **Folders:** *Duet sounds* (the built-in drum kit as WAV files), *My samples*
  (yours: right-click to make folders, *Import…* copies sounds from your
  computer in) and *+ Folder on disk* to show your own sample packs where they
  are.
- **Bins:** your own collections, as in Premiere or Avid: *+ Bin*, then drag
  sounds onto it, from any folder. Bins can hold bins (right-click: *New bin
  inside*). Taking a sound out of a bin leaves the file alone.
- **Search** finds sounds in every folder by name; *Recent* has the ones you
  used last.
- **Click** a sound to hear it (just you) and see its waveform.
- **Drag** it into the playlist (a clip on that track, or a new track below
  the last one), onto a drum pad or a step row of a drum channel (that pad's
  sound), onto a sampler channel (its sound), or onto empty space in the
  channel rack: a new **sampler** channel that plays the sound at every pitch
  (C4 plays it as it is; *One-shot* plays a hit to its end).

A sound you use in the song is copied into the project, so your partner gets
it too and hears the same.

- **Kinds of files:** WAV, AIFF, FLAC, MP3, M4A (AAC or Apple Lossless, as an
  iPhone records them) and Ogg Vorbis. A file stays the kind it is in the
  project, so an M4A stays small on its way to your partner.
- **Long files** (an hour-long recording, a whole mix) load in the
  background: the clip says *Loading…* until it's ready, and Duet keeps
  answering meanwhile. On a Mac the copy into the project is an instant APFS
  clone that takes no extra space. A file at another sample rate (44.1 kHz on
  a 48 kHz interface) is converted as it plays, so there's no waiting for a
  converted copy either.

## Instruments

A strip of yours can play an **instrument** instead of an input: in the strip's
input menu (where it says *In 1*) pick **Drum kit**, **Synth** or **VST
instrument…**. Your partner hears it like any other channel, its effects work
as usual, and a song track set to that strip records it.

- **Drum kit** (built in): 16 pads on the General MIDI drum notes (36–51: kick,
  snares, clap, hats, toms, crash, ride), so drum parts and pad controllers land
  on the right sound. Click a pad to hit it (higher up = harder). *⋯* on a pad
  loads your own WAV onto it. Knobs: level, tune, decay.
- **Synth** (built in): 8 voices, saw/square blend, a resonant filter, attack,
  release and detune. Sustain pedal works.
- **VST3 instruments**: the instruments in your VST3 folder; *Open window* shows
  their own controls. Like VST effects in a song, your partner only needs the
  plugin for a song track's effects, not to hear you play it live.

**Playing it:**
- A **MIDI keyboard**: pick it under *Sound → MIDI keyboard* (Duet opens it again
  by itself next time, also when you plug it back in).
- Your **computer keyboard**: press **Keys** (top right). The row *A S D F G H J
  K L ;* are the white keys, *W E T Y U O P* the black ones; *Z / X* move an
  octave down / up. While Keys is on, those letters play notes instead of the
  playlist shortcuts (space still plays and stops).

Both play the **selected** strip's instrument (click a strip), or else your first
strip that has one.

Effects on your partner's own strips are part of what they send: you hear them
without doing anything, and their strip shows which ones ("theirs: …").
Effects you add on their strip only change your own mix.

Everything you set up is remembered for next time, plugin settings included.

**Video:** press *Camera* (top right) to turn yours on; your partner's shows up
when they do the same. Drag the video panel by its title bar; the arrows button
makes it big. Video runs separately from the audio (it's about 0.1–0.2 s behind
it, which is normal), and it's capped at 0.8 Mbit/s so it can't crowd out the sound.

The big number top right is the delay from your partner's instrument to your
ears; the bar at the bottom shows where it comes from.

## Getting the latency down

- **Ethernet cable, not Wi-Fi**, on both ends.
- **Headphones**, so your mic doesn't pick your partner up again.
- **ASIO** with a real audio interface on Windows; lower the buffer in its control
  panel (Focusrite Control → 64 or 32).
- Distance sets the floor: about 1 ms per 100 km each way plus routing.

## When Connect doesn't work

Most home routers allow hole punching; some don't (symmetric NAT on some 4G/5G
and carrier-grade NAT). Try another connection (phone tether), or port forward
on one side: start with `--port 50555`, forward UDP 50555 to that computer, and
have the other person enter `public.ip:50555` as the partner code.

## Development

Needs Python 3.9+ and Rust (https://rustup.rs). On Windows also LLVM (for the
ASIO bindings: `winget install LLVM.LLVM`, then `set LIBCLANG_PATH=C:\Program Files\LLVM\bin`).

```
python -m venv .venv && .venv\Scripts\activate     # Mac: source .venv/bin/activate
pip install -r requirements.txt maturin
pip install ./engine      # compiles the Rust audio engine (or: cd engine && maturin develop --release)
python duet.py            # --debug opens the web inspector
python selftest.py        # no sound card needed; --stun also checks the address lookup
cd engine && cargo test --release   # engine unit tests
cargo build --release --example test_gain --example test_synth   # test plugins the self-test uses
pyinstaller --noconfirm duet.spec   # build Duet.exe / Duet.app for this OS
```

Open `ui/index.html?demo` in a browser to work on the interface with a fake backend.

**How it's split:** Python + the web page do setup and display. Everything
time-critical is in the Rust engine, on native threads that never wait for Python:
the audio callback never locks, allocates or touches the GIL; other threads talk
to it through lock-free queues and atomics.

| File | What |
|---|---|
| `engine/src/dsp.rs` | The per-block audio work: effects, send, jitter buffer playout, loss concealment, mix, master rack |
| `engine/src/mix.rs` | The song's mixer: track strips, buses, sends, sidechain keys, delay compensation (playback and export share it) |
| `engine/src/audio.rs` | Sound card via cpal (ASIO / CoreAudio / WASAPI), on one COM-STA thread |
| `engine/src/jitter.rs` | Jitter buffer: reorder, loss detection, burst-tolerant clock-drift trim |
| `engine/src/fx.rs` | FX racks (stereo): EQ, compressor with sidechain, look-ahead limiter, overdrive, chorus, tremolo, delay, Freeverb, NAM amp (nam-rs) |
| `engine/src/net.rs` | Packet format, connect codes, STUN |
| `engine/src/signal.rs` | Reliable side channel for the video call setup |
| `engine/src/instr.rs` | Instruments: the synthesized drum kit, the synth, VST3 instruments (notes in) |
| `engine/src/midi.rs` | MIDI keyboards (midir), on a thread of their own |
| `engine/src/daw.rs` | Song timeline: clips (converted to the device rate as they play), the fast WAV reader, waveform peaks, take writer |
| `engine/src/stems.rs` | The stem splitter: Demucs's spectrogram, its network in ONNX Runtime, the overlap-add (matches PyTorch Demucs to 2e-5) |
| `tools/export_demucs.py` | Exports Demucs's network to ONNX (CI runs it for a commit with `[models]` and publishes the release `models-htdemucs-1`) |
| `engine/src/stretch.rs` | Keep-pitch speed and transposing (Signalsmith Stretch), and finding a clip's hits, tempo and peak |
| `ui/clipedit.js` | The clip editor (double-click an audio clip) |
| `ui/keys.js` | Keyboard shortcuts: the actions, the sets in other programs' style, the settings window |
| `engine/src/decode.rs` | Every other kind of sound file (MP3, M4A/AAC/ALAC, FLAC, AIFF, Ogg) through symphonia |
| `engine/src/transfer.rs` | Reliable, paced file transfer to the partner (recorded takes) over the audio socket |
| `engine/src/livenotes.rs` | Notes played live on a song track, to the partner: tiny packets with the last few events repeated, so loss can't leave a note hanging |
| `engine/src/lib.rs` | Network threads (receive, punching, ping, files) and the Python API |
| `duet.py` | Window (pywebview) and the API the page calls |
| `daw.py` | The shared song: a last-writer-wins document synced with the partner, recording, the engine's timeline |
| `t3k.py` | TONE3000: OAuth/PKCE tone picker over a loopback redirect, model download, local library |
| `ui/index.html` | The interface, incl. the WebRTC video call |
| `ui/wm.js` | The workspace: windows that move, size and snap, layouts |
| `ui/daw.js`, `ui/daw.css` | The playlist: transport, track headers, timeline with waveforms, song menu |
| `ui/seq.js`, `ui/seq.css` | The sequencer: patterns, channel rack with steps, piano roll with velocities |
| `ui/instr.js`, `ui/instr.css` | Instruments: the panel with pads and knobs, the computer keyboard, MIDI keyboards |
| `ui/presence.js` | Seeing each other work: the partner's pointer, selection, view and live notes; Follow |
| `library.py` | The sample library: folders, bins, search, import |
| `ui/auto.js` | Automation lanes in the playlist: drawing them, *Automate* on knobs and faders |
| `ui/browser.js` | The browser on the left: folders, bins, preview, drag sounds into the song |
| `ui/mixer.js` | Buses, FL-style routing (arrows, cables, sends, live strips' sends too), effect windows, the sidechain picker, the EQ's curve |
| `.github/workflows/build.yml` | Every push: engine tests + self-test on Linux. The Mac apps (built, tested, smoke-tested) for main, or on request: Actions → build → Run workflow |

Live audio is uncompressed 16-bit mono, 48 kHz: about 800 kbit/s each way.
Recorded takes are 24-bit WAV; they go to the partner after recording at up to
about 12 Mbit/s, slowing down by themselves when the line gets busy. Files are
read and written a chunk at a time on both sides, so a file of any size goes
across without having to fit in memory (an hour of 24-bit stereo, about 1 GB,
takes 10 to 15 minutes at full speed). The wire
format hasn't changed since the first (Python) version, so old and new builds
can still talk to each other.