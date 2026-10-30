# Calibration tools (stage 9)

Compare the model with a recording of a real floor tom, partial by partial.

```
# 1. a recording (one hit, a mono or stereo WAV) -> its partials
python tools/analyze.py tom_hit.wav --out tom_hit.json

# 2. the model's modes, as heard by the same output, for a strike at the same place (r as a fraction of R, θ)
cargo run --release --example mode_table -- --json --output close --strike 0.5,0 > model.json

# 3. side by side: a table, and a picture (frequency against T60, and both spectrograms)
python tools/compare.py tom_hit.json model.json --wav-a tom_hit.wav --wav-b model_close.wav --out compare.png

# 4. try new parameters: a JSON with only what changes (anything else keeps its default)
echo '{"batter": {"tension": 1400}, "air_mass_factor": 1.6}' > p.json
cargo run --release --example render -- --params p.json --hit 0.7,0.5,0 --seconds 4 --out renders/try
cargo run --release --example mode_table -- --json --params p.json --output close > model.json
```

`analyze.py` takes a Blackman-Harris STFT of the tail (from 30 ms after the loudest sample), picks the peaks
of its first frames, follows each one (it may glide), and fits its level in dB with a line until it nears the
noise floor: T60 = −60/slope; the level is extended back to the onset, and the strongest partial has amp 1.
`tools/test_tools.py` checks both scripts on a signal with known partials. They need numpy, scipy, matplotlib.

What to record (spec §7): one floor tom close to the default (16"×16"), heads and their pitches noted; 10 strike
radii from the centre to 0.9 R on one angle between lugs, 5 velocities each, 3 takes; a close mic like the model's
(half the radius, 6 cm up) and a room mic at 1.5 m; and the whole set again with the resonant head damped.

## What the tools say about the model itself (renders/stage9)

A hit rendered by the model, analysed, and compared with the model's own mode table, closes the loop:

* **close**: 21 of 21 partials found, median pitch error 0.9 cents, T60 error ×1.00. The chain works.
* **far**: the axisymmetric modes match (T60 ×1.0, 10–15 cents sharp: the tension modulation is still raised
  while the head rings), but there are 10 partials more, at the pairs' frequencies, with half their T60. The far
  output only hears the axisymmetric modes, and the tension modulation (quadratic) drives those at the pairs'
  frequencies and their sums and differences. That is the model's nonlinearity, not a fault of the tools.

## A first calibration: a Pearl 16" floor tom (presets/pearl16.json)

The recordings: the AVL Drumkits "Black Pearl" kit by Glen MacArthur (bandshed.net, CC-BY-SA 3.0), its
16" floor tom, centre and edge hits, 5 velocities each, 44.1 kHz mono. Used for measuring only: the samples
are not in this repository.

Measured: centre hits 69 Hz (T60 2.3 s, gliding down ~130 cents); edge hits 68 / 110 / 158 Hz (T60 2.8 / 2.1 /
1.6 s), so (0,1) / (1,1) / (2,1); and strong partials between 1.0 and 1.7 kHz with T60 0.3–0.4 s.

Fitted (a grid over the batter's tension and the air-mass factor, the resonant head kept at the same ratio of
tension; σ1 from the high partials' decay):

| | measured | model, presets/pearl16.json |
|---|---|---|
| centre: lowest partial | 69 Hz, T60 2.31 s, glide 130 c | 69 Hz, T60 2.24 s, glide 167 c (vel 0.7) |
| edge: (0,1), (1,1), (2,1) | 68 / 110 / 158 Hz | 69 / 114 / 155 Hz (37 cents rms) |
| edge: T60 of those | 2.8 / 2.1 / 1.6 s | 2.2 / 2.2 / 2.1 s |
| 1.0–1.7 kHz | strong, T60 0.3–0.4 s | nearly nothing |

The low end is close. The model has none of the recording's 1–1.7 kHz content (the attack and body of a real
tom): the close mic's e^{−κh} weighting leaves the high modes out, and the far output hears only m = 0. The
(2,1)/(1,1) ratio would need an air-mass factor of ~5 to match; β_air = 0.8 fits (0,1) better, so the fit
compromises. With the lower tension more modes fall under f_max (1207 at `High`): use `Normal`.
