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
   mix them however you like. **Safety buffer** (top right): on **Auto** it
   finds the smallest buffer that doesn't crackle by itself: up a step on a
   dropout, down a step after 10 s without trouble (on a quiet moment, so you
   don't hear it). − / + set it by hand instead; each step is one block (about
   2.7 ms at 128 / 48k).
4. **FX rack:** click a strip to see its effects below. *+ Add effect* adds a
   pedal, the arrows reorder, ✕ removes, the round footswitch turns it on/off.
   Drag knobs up/down (Shift = fine, double-click = reset, scroll wheel works).
   Presets replace the strip's whole rack. Effects on your strips are heard by
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
7. **Metronome** (next to *Mixer*): on/off, tempo (type it, −/+, or *Tap*) and
   beats per bar are shared with your partner; the click volume is just yours
   and the click is never sent. Whoever starts it or changes the tempo leads;
   the other one's click is lined up with the leader's playing *as it arrives*,
   so you play along to what you hear. Tempo-synced plugins follow it too.

## Recording a song together

The **Playlist** (top left, next to *Mixer*) is a timeline you both work in at
the same time, like a shared document: the song is open on both computers and
every change (a new track, a moved clip, the tempo) shows up on the other side
right away. The green dot on the song name means you're in the same song.

1. **Tracks:** *+ Track* adds one. Under its name you pick **who plays on it and
   from which input**: *You · Guitar* or *Partner · Vocals*. Orange tracks are
   yours, teal ones your partner's.
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

Click in the ruler to move the playhead, drag in the ruler to mark a loop.
*Space* plays and stops, *R* records, *Ctrl + scroll* zooms. *Click* gives you
a metronome while the song plays (just for you).

**Export** (top right of the playlist) saves the song as a 24-bit WAV: just the
mix, or the mix plus every track on its own (stems, e.g. to mix it further in
another program), the whole song or just the loop. It renders faster than real
time and the sound keeps running meanwhile. What you hear is what you get: the
faders, pans, effects, mute and solo as you have them.

Each track also has a strip in the **Mixer** (under *Song*) with volume, pan
and its own FX rack, NAM amps and VST3 plugins included; *FX* on a track header
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
- **Routing:** a song track's FX rack starts with a **Routing** box. *Output*
  sends the track to a bus instead of the master (the strip then says
  *→ Drums*); the knobs below are **sends**: some of the track to a bus, after
  its fader, on top of its own sound. Up to four per track. A bus can go to
  another bus, as long as it doesn't loop back.
- **Sidechain:** a compressor on a song track or the master can listen to
  another track instead of its own sound (*Sidechain:* under its knobs): the
  kick pushes the bass down.
- **Master FX:** click the *Master* strip: its rack shapes the whole song (an EQ
  and a limiter at the end make it fuller and louder). It's on what you hear
  live too.
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

## Sequencer: patterns, channel rack, piano roll

The **Sequencer** tab works like FL Studio's channel rack and piano roll, in
the shared song (your partner sees every note you place, and you theirs).

- **Channels** are the song's instruments: *+ Channel* → Drum kit, Synth or a
  VST instrument. A channel is also a track in the playlist and a strip in the
  mixer (*Sound* opens its knobs, pads and effects there).
- **Patterns** hold notes for any number of channels. Pick one at the top (or
  *+ New pattern*), set its length (1–8 bars); *⋯* renames, copies, clears or
  deletes it.
- **Steps:** every channel has a row of 16th-note steps per bar; a drum kit has a
  row per pad (*All pads* shows all 16). Click a step to switch it on or off.
- **Piano roll** (below) for the selected channel: click to draw a note, drag to
  move it (up/down changes the pitch), drag its right end to make it longer,
  right-click to delete. Notes snap to the grid (*Snap*, or hold *Alt*). The bars
  underneath are the velocities: drag them up or down. Click a key on the left to
  hear it. *Ctrl+A*, *Delete* and *Ctrl+Z* work here too.
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

## Instruments

A strip of yours can play an **instrument** instead of an input: in the strip's
input menu (where it says *In 1*) pick **Drum kit**, **Synth** or **VST
instrument…**. Your partner hears it like any other channel, its FX rack works
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
| `engine/src/daw.rs` | Song timeline: clips, audio files (WAV in/out, rate conversion, waveform peaks), take writer |
| `engine/src/transfer.rs` | Reliable, paced file transfer to the partner (recorded takes) over the audio socket |
| `engine/src/livenotes.rs` | Notes played live on a song track, to the partner: tiny packets with the last few events repeated, so loss can't leave a note hanging |
| `engine/src/lib.rs` | Network threads (receive, punching, ping, files) and the Python API |
| `duet.py` | Window (pywebview) and the API the page calls |
| `daw.py` | The shared song: a last-writer-wins document synced with the partner, recording, the engine's timeline |
| `t3k.py` | TONE3000: OAuth/PKCE tone picker over a loopback redirect, model download, local library |
| `ui/index.html` | The interface, incl. the WebRTC video call |
| `ui/daw.js`, `ui/daw.css` | The playlist: transport, track headers, timeline with waveforms, song menu |
| `ui/seq.js`, `ui/seq.css` | The sequencer: patterns, channel rack with steps, piano roll with velocities |
| `ui/instr.js`, `ui/instr.css` | Instruments: the panel with pads and knobs, the computer keyboard, MIDI keyboards |
| `ui/presence.js` | Seeing each other work: the partner's pointer, selection, view and live notes; Follow |
| `ui/mixer.js` | Buses, routing and sends, the sidechain picker, the EQ's curve |
| `.github/workflows/build.yml` | Every push: engine tests + self-test on Linux. The Mac apps (built, tested, smoke-tested) for main, or on request: Actions → build → Run workflow |

Live audio is uncompressed 16-bit mono, 48 kHz: about 800 kbit/s each way.
Recorded takes are 24-bit WAV; they go to the partner after recording at up to
about 12 Mbit/s, slowing down by themselves when the line gets busy. The wire
format hasn't changed since the first (Python) version, so old and new builds
can still talk to each other.