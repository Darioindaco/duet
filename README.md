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
   reverb on their vocals).

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
   delay. Instruments aren't listed (racks have no MIDI). The first scan takes
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

In the playlist: click a clip to select it (shift-click for more, ctrl+A for
all), drag it to move it (also to another track; it snaps to the grid, hold *Alt*
to move freely), *Delete* removes it. Click in the ruler to move the playhead,
drag in the ruler to mark a loop. *Ctrl + scroll* zooms. *Click* gives you a
metronome while the song plays (just for you).

Each track also has a strip in the **Mixer** (under *Song*) with volume, pan
and its own FX rack, NAM amps and VST3 plugins included; *FX* on a track header
takes you there. Volume, pan and effects are part of the song (you both hear
them); mute and solo are just for you. A VST on a song track needs that plugin
on both computers.

Songs are saved by themselves. The song name opens the song menu: rename it,
start a new one, or open an older one (your partner switches along). If you
both had a different song open when you connected, Duet asks which one to use;
the other one stays saved.

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
pyinstaller --noconfirm duet.spec   # build Duet.exe / Duet.app for this OS
```

Open `ui/index.html?demo` in a browser to work on the interface with a fake backend.

**How it's split:** Python + the web page do setup and display. Everything
time-critical is in the Rust engine, on native threads that never wait for Python:
the audio callback never locks, allocates or touches the GIL; other threads talk
to it through lock-free queues and atomics.

| File | What |
|---|---|
| `engine/src/dsp.rs` | The per-block audio work: effects, send, jitter buffer playout, loss concealment, mix |
| `engine/src/audio.rs` | Sound card via cpal (ASIO / CoreAudio / WASAPI), on one COM-STA thread |
| `engine/src/jitter.rs` | Jitter buffer: reorder, loss detection, burst-tolerant clock-drift trim |
| `engine/src/fx.rs` | FX racks: compressor, overdrive, chorus, tremolo, delay, Freeverb, NAM amp (nam-rs) |
| `engine/src/net.rs` | Packet format, connect codes, STUN |
| `engine/src/signal.rs` | Reliable side channel for the video call setup |
| `engine/src/daw.rs` | Song timeline: clips, audio files (WAV in/out, rate conversion, waveform peaks), take writer |
| `engine/src/transfer.rs` | Reliable, paced file transfer to the partner (recorded takes) over the audio socket |
| `engine/src/lib.rs` | Network threads (receive, punching, ping, files) and the Python API |
| `duet.py` | Window (pywebview) and the API the page calls |
| `daw.py` | The shared song: a last-writer-wins document synced with the partner, recording, the engine's timeline |
| `t3k.py` | TONE3000: OAuth/PKCE tone picker over a loopback redirect, model download, local library |
| `ui/index.html` | The interface, incl. the WebRTC video call |
| `ui/daw.js`, `ui/daw.css` | The playlist: transport, track headers, timeline with waveforms, song menu |
| `.github/workflows/build.yml` | Builds, tests and smoke-tests the Mac app on every push |

Live audio is uncompressed 16-bit mono, 48 kHz: about 800 kbit/s each way.
Recorded takes are 24-bit WAV; they go to the partner after recording at up to
about 12 Mbit/s, slowing down by themselves when the line gets busy. The wire
format hasn't changed since the first (Python) version, so old and new builds
can still talk to each other.