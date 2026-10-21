# Duet

Free peer-to-peer jamming for two people. Audio goes straight from one computer
to the other over UDP: no server, no account, no subscription.

## Getting it running

**Windows, easiest:** send your partner `dist\Duet.exe` (one file, about 24 MB) and
double-click it. Windows SmartScreen will warn because the file isn't signed:
click *More info → Run anyway*. On first launch, allow it through the Windows
firewall (private *and* public networks).

**Mac:**

1. Install Python from https://www.python.org/downloads/macos/ (once). Apple's built-in
   `python3` has a broken window toolkit, so use the python.org one.
2. Unzip `duet-mac.zip` into Downloads, open **Terminal** and run:
   ```
   bash ~/Downloads/duet/run_mac.command
   ```
   The first run takes about a minute to set up. Use the same command every time after that.
3. When macOS asks whether **Terminal may use the microphone**, click Allow. If you
   missed it: System Settings → Privacy & Security → Microphone → turn on Terminal,
   then restart Duet. When it asks about **incoming network connections**, click Allow.

On a Mac there's no ASIO. Pick the audio interface by name, or the built-in mic
and headphones. With an interface, set its sample rate to 48 kHz in *Audio MIDI
Setup* if 48000 won't start.

**From source (Windows / Mac / Linux):**

```
python -m venv .venv
.venv\Scripts\activate          # Mac/Linux: source .venv/bin/activate
pip install -r requirements.txt
python duet.py
```

## Using it

1. **Sound:** pick your audio interface (choose the `[ASIO]` entry on Windows),
   the input channel your instrument/mic is on, and the output. Use the **same
   buffer and sample rate on both sides** (128 / 48000 is a good start). Press
   *Start audio*.
2. **Connect:** press *Copy* next to *Your code* and send it to your partner (in
   WhatsApp, anywhere). Paste theirs into *Partner's code* and press *Connect*.
   **Both** of you press Connect. That's what punches through your routers.
3. **Mix:** *Partner volume*, *Hear myself* (leave at 0 if your interface has
   direct monitoring, which has zero delay), and *Safety buffer*. Raise the safety
   buffer one step at a time if you hear crackles or dropouts. Each step adds
   about 2.7 ms.

The bottom line shows the estimated delay from your partner's instrument to your
ears, broken down into network, safety buffer and sound card.

## Getting the latency down

- **Ethernet cable, not Wi-Fi**, on both ends. Wi-Fi jitter forces a bigger safety buffer.
- **Headphones.** With speakers your mic picks up your partner, and they hear themselves back.
- **ASIO** with a real audio interface. Without one, use the `[WASAPI]` entry; it
  works, but expect 20–40 ms more.
- Lower the buffer in your interface's own control panel (Focusrite Control → 64 or 32).
- Distance sets the floor: light in fibre needs about 1 ms per 100 km each way,
  plus routing. Same country usually feels fine; across an ocean you'll feel it.

## When Connect doesn't work

Most home routers allow hole punching. Some don't, e.g. "symmetric NAT" on a few
mobile/4G connections and carrier-grade NAT. Fixes, simplest first:

- Try the other person's internet (or tether to a phone).
- **Port forward** on one side: start with a fixed port, `python duet.py --port 50555`
  (or `Duet.exe --port 50555`), forward UDP 50555 on that router to that computer,
  and have the other person type `your.public.ip:50555` as the partner code.
- Same house / same Wi-Fi works without any of this; the code includes your LAN address.

## How it works

| File | What |
|---|---|
| `net.py` | STUN lookup of your public address (free public servers), connect codes, packet format |
| `engine.py` | UDP link, hole punching, ping, jitter buffer (reorder, loss concealment, clock-drift trim) |
| `duet.py` | Tkinter UI + sound card stream (sounddevice / PortAudio, ASIO on Windows) |
| `selftest.py` | Tests without a sound card: jitter buffer, codes, two engines over localhost (`--stun` to check the address lookup) |

Audio is uncompressed 16-bit mono, 48 kHz: about 800 kbit/s each way. Fine on
any normal broadband connection.

Rebuild the exe: `pip install pyinstaller` then
`pyinstaller --noconfirm --onefile --windowed --name Duet --collect-data _sounddevice_data duet.py`
