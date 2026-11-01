# Third-party components

Duet is proprietary (see LICENSE). It is built on the open-source components below. Each one
keeps its own license; their notices must ship with the app. The full list of transitive Rust
crates can be produced with `cargo about` or `cargo license` in `engine/`.

| Component | What it does in Duet | License |
|---|---|---|
| PyO3 | Python ↔ Rust bridge | MIT OR Apache-2.0 |
| cpal (+ asio-sys on Windows) | audio devices | Apache-2.0 |
| rtrb | lock-free queues to the audio thread | MIT OR Apache-2.0 |
| nam-rs | Neural Amp Modeler playback | MIT |
| vst3 (coupler-rs) | VST3 hosting bindings | MIT OR Apache-2.0 |
| Signalsmith Stretch (ssstretch) | time-stretch and pitch-shift | MIT |
| ONNX Runtime (via ort) | runs the stem-splitting model | MIT |
| Demucs (Meta) | stem-splitting model | MIT |
| RustFFT / realfft | spectra, analysis | MIT OR Apache-2.0 / MIT |
| Symphonia | MP3, AAC, FLAC, AIFF, Ogg decoding | MPL-2.0 (used unmodified) |
| hound | WAV reading and writing | Apache-2.0 |
| midir | MIDI input | MIT |
| libloading, base64, serde_json, rand | plumbing | ISC / MIT OR Apache-2.0 |
| pywebview | the app window | BSD-3-Clause |
| certifi | HTTPS certificates | MPL-2.0 |

Notes before selling:

- **ASIO** (Windows): the ASIO SDK is Steinberg's. Shipping an ASIO-enabled build needs Steinberg's
  ASIO license agreement (or their GPLv3 option, which a closed app can't use).
- **VST3 / Audio Units**: hosting needs the VST3 SDK terms (Steinberg) and Apple's AU APIs; the
  "VST" name and logo have their own usage guidelines.
- **MPL-2.0** (Symphonia, certifi): fine in a closed app as long as those files themselves are
  not modified, or their modified source is published.
