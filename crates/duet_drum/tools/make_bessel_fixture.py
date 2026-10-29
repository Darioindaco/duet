"""Writes tests/data/bessel_fixture.json: Bessel zeros and values from scipy,
the reference the Rust implementation (src/bessel.rs) is tested against.

    python tools/make_bessel_fixture.py        (needs numpy, scipy)
"""
import json
from pathlib import Path

import numpy as np
from scipy.special import jn_zeros, jv, jvp

M_MAX, N_MAX = 100, 30
zeros = {str(m): [float(z) for z in jn_zeros(m, N_MAX)] for m in range(M_MAX + 1)}
# values and derivatives on a grid that crosses the regimes (x < m, x ~ m, x > m)
vals = []
for m in (0, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89):
    for x in (0.0, 1e-3, 0.5, 1.0, 2.4048255576957727, 5.0, 10.0, 17.3, 30.0, 55.5, 80.0, 99.9, 120.0):
        vals.append([m, x, float(jv(m, x)), float(jvp(m, x))])
out = Path(__file__).resolve().parent.parent / "tests" / "data" / "bessel_fixture.json"
out.write_text(json.dumps({"n_max": N_MAX, "zeros": zeros, "values": vals}))
print(f"wrote {out} ({out.stat().st_size // 1024} KiB)")
