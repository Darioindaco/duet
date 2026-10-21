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

1. **Sound:** pick your audio interface (on Windows the `ASIO` entry), the input
   your instrument/mic is plugged into, and the output. **Same buffer and sample
   rate on both sides** (128 / 48k is a good start). Press *Start audio*.
2. **Connect:** *Copy* your code and send it to your partner; paste theirs and
   press *Connect*. **Both** of you press Connect. That's what gets through your routers.
3. **Mix:** *Hear me* (your own sound, with effects), partner *Volume*, *Room*
   (reverb on your partner), and the *Safety* buffer: press + if you hear
   crackles, each step adds about 2.7 ms.
4. **Pedalboard:** stomp the round footswitch to turn a pedal on; drag knobs up/down
   (Shift = fine, double-click = reset, scroll wheel works too). Presets at the top.
   Your partner hears your effects. To hear them yourself, turn up *Hear me*
   and turn off your interface's direct monitoring.

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
| `engine/src/fx.rs` | Pedalboard: compressor, overdrive, chorus, tremolo, delay, Freeverb |
| `engine/src/net.rs` | Packet format, connect codes, STUN |
| `engine/src/signal.rs` | Reliable side channel for the video call setup |
| `engine/src/lib.rs` | Network threads (receive, punching, ping) and the Python API |
| `duet.py` | Window (pywebview) and the API the page calls |
| `ui/index.html` | The interface, incl. the WebRTC video call |
| `.github/workflows/build.yml` | Builds, tests and smoke-tests the Mac app on every push |

Audio is uncompressed 16-bit mono, 48 kHz: about 800 kbit/s each way. The wire
format hasn't changed since the first (Python) version, so old and new builds
can still talk to each other.