# duet_drum

A physically modelled floor tom (v1) by nonlinear modal synthesis: the linear physics (two membranes, air
loading, the cavity, uneven lug tension) is solved once at prepare time into a bank of uncoupled
oscillators; the stick's contact and the heads' tension modulation are integrated with an energy-stable
scalar auxiliary variable (SAV) scheme. Pure synthesis: no samples, bit-identical output for the same
parameters and hits.

```
cargo test --release                       # 46+ tests across the stages
cargo run --release --example stage8_report
cargo run --release --example stage7_render   # renders/stage7/*.wav (not in git)
```

## Stages (each has tests/stageN.rs and examples/stageN_report.rs)

1. Bessel functions, mode basis, parameters. 2. Linear single head, exact modal discretisation.
3. The stick (SAV). 4. Tension modulation. 5. Two heads, air loading, cavity, radiation damping.
6. Lugs. 7. Outputs (close mic, far monopole), gains, block interface. 8. Performance.

## Where this deviates from the spec (and why)

* **Contact stiffness `K_s = 1e6`** (spec 1e8): 1e8 chatters and isn't resolved at audio rates (stage-3 report).
* **`contact_radius = 10 mm`** (new): the stick drives a Gaussian patch, so it hardly drives the highest modes.
  Without it the contact force ripples at ~3 kHz, a buzz at every strike.
* **`attack_lowpass_hz = 800`** (new): a 4-pole low-pass on the contact force before the close mic's dipole term
  differentiates it. 0 gives the spec's raw difference.
* **`shell`, `shell_edge_boost`** (new, roadmap §8.3): the shell's modes, driven by the contact force. Empty by
  default; `presets/pearl16.json` has nine, measured from a real 16" floor tom.
* **`Quality`**: the spec's cut-offs (2500/1500 Hz) are `High`; `Normal`/`Draft` keep fewer modes.

Set `contact_radius = 0`, `attack_lowpass_hz = 0`, `stick.stiffness = 1e8` for the spec's behaviour.

## Known limits of v1

The far output hears only the axisymmetric modes (a monopole), so the (1,1) pair, the main tone of a tom,
is not in it; the close mic is dominated by the contact-force term. The pairs ring for ~2 s (only block A
radiates). Contact lasts 7-9 ms, longer than a real stick. Tension modulation reaches ΔT/T ≈ 80 % at the hardest
strike. All of these are calibration or roadmap items (spec §8), not bugs.
