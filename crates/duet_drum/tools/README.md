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
