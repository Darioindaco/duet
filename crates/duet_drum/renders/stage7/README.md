# duet_drum, stage 7 renders

48 kHz, mono, 32-bit float, `cargo run --release --example stage7_render`. Every file comes as `*_close.wav` (the close mic, 6 cm above the batter at half the radius) and `*_far.wav` (the monopole at 1.5 m). The output gains are the defaults (a velocity-1.0 hit at the centre peaks at −6 dBFS); nothing is normalised, so the levels between files are comparable.

## grid_close.wav, grid_far.wav

Sixteen hits, one every 1.5 s (a later hit rings into the end of the one before; the head is the same voice), velocity-major.

| at | velocity | radius (× R) |
|---|---|---|
|  0.0 s | 0.15 | 0 |
|  1.5 s | 0.15 | 0.3 |
|  3.0 s | 0.15 | 0.6 |
|  4.5 s | 0.15 | 0.85 |
|  6.0 s | 0.4 | 0 |
|  7.5 s | 0.4 | 0.3 |
|  9.0 s | 0.4 | 0.6 |
| 10.5 s | 0.4 | 0.85 |
| 12.0 s | 0.7 | 0 |
| 13.5 s | 0.7 | 0.3 |
| 15.0 s | 0.7 | 0.6 |
| 16.5 s | 0.7 | 0.85 |
| 18.0 s | 1 | 0 |
| 19.5 s | 1 | 0.3 |
| 21.0 s | 1 | 0.6 |
| 22.5 s | 1 | 0.85 |

## roll_close.wav, roll_far.wav

Sixteen strokes at 12 Hz, velocity 0.7, radius alternating 0.3 / 0.35, each at a new angle; one voice, so every stroke lands on a head still ringing from the last.

## lugs_even, lugs_plus3pct, lugs_plus30pct

The same hit (velocity 0.7, radius 0.6, 5 s) with all lugs even, with one lug 3 % tighter (the spec) and with one 30 % tighter. The splits are 0.1 Hz and 1 Hz for the (1,1) pair, so only the last one has a chance of being heard as a beat (stage-6 report).

