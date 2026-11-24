"""Generate the synthetic CSV samples in tests/data/.

Nothing here was recorded from hardware. The generators are deterministic
(fixed seeds) and are also imported by the tests, which know the ground truth
(noise levels, sensor gains, knock position, wave speed).

    uv run python tests/make_data.py     # rewrite tests/data/*.csv
"""

from __future__ import annotations

from pathlib import Path

import numpy as np

DATA_DIR = Path(__file__).parent / "data"

PIEZO_POS_CM = np.array([[-5.0, 3.33], [5.0, 3.33], [5.0, -3.33], [-5.0, -3.33]])
SENSOR_POS_CM = np.array(
    [[-10, 6.67], [0, 6.67], [10, 6.67], [10, 0], [10, -6.67], [0, -6.67], [-10, -6.67], [-10, 0], [0, 0]]
)

#: Ground truth used by the tests.
T06_RMS_MG = [2.0 + 0.5 * s for s in range(9)]
TRUE_GAINS = np.array([1.00, 0.90, 1.10, 0.95, 1.05, 0.90, 1.00, 0.95, 1.15])
T10_POS_CM = (2.0, 1.0)
T10_V_M_S = 500.0
SKEW_US = 2.5


def t06_noise(samples: int = 200, seed: int = 6) -> str:
    """Nine stationary sensors; sensor s has RMS noise T06_RMS_MG[s] on every axis."""
    rng = np.random.default_rng(seed)
    lines = ["# t06_noise,sensor,sample,x_mg,y_mg,z_mg"]
    for s in range(9):
        noise = rng.normal(0.0, 1.0, (samples, 3))
        # Normalise so the sample RMS is exactly the target.
        noise = (noise - noise.mean(axis=0)) / noise.std(axis=0) * T06_RMS_MG[s]
        xyz = noise + np.array([10.0 * s, -5.0, 1000.0])
        for i, (x, y, z) in enumerate(xyz):
            lines.append(f"{s},{i},{x:.3f},{y:.3f},{z:.3f}")
    lines.append("# end")
    return "\n".join(lines) + "\n"


def t07_capture(
    near: int | None,
    knocks: int = 3,
    seed: int = 7,
    with_peaks: bool = True,
    samples: int = 20,
    strength_mg: float = 2000.0,
) -> str:
    """Knocks next to sensor `near` (None: at the plate centre).

    Sensor i responds with strength / TRUE_GAINS[i] * falloff(distance), so
    that the calibrated responses are equal for equal distances.
    """
    rng = np.random.default_rng(seed + (near if near is not None else 99))
    pos = SENSOR_POS_CM[near] if near is not None else np.array([0.0, 0.0])
    lines = ["# t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg"]
    for k in range(1, knocks + 1):
        strength = strength_mg * (1.0 + 0.02 * rng.normal())
        peaks = []
        for s in range(9):
            d = float(np.linalg.norm(SENSOR_POS_CM[s] - pos))
            amp = strength / TRUE_GAINS[s] / (1.0 + (d / 6.0) ** 2)
            peaks.append(amp)
            for i in range(samples):
                # 4 quiet pre-trigger samples at 1 g, then a decaying z burst
                # whose first sample is the peak.
                z = 1000.0
                if i >= 4:
                    z += amp * (-0.6) ** (i - 4)
                lines.append(f"{k},{s},{i},0.000,0.000,{z:.3f}")
        if with_peaks:
            lines.append(f"# knock {k} peaks," + ",".join(f"{p:.1f}" for p in peaks))
    lines.append("# end")
    return "\n".join(lines) + "\n"


def _burst(t_us: np.ndarray, arrival_us: float, amp: float) -> np.ndarray:
    """Damped 5 kHz sine starting at `arrival_us`."""
    dt = np.clip(t_us - arrival_us, 0.0, None)
    return amp * np.sin(2 * np.pi * 5e-3 * dt) * np.exp(-dt / 600.0) * (t_us >= arrival_us)


def piezo_arrivals_us(pos_cm=T10_POS_CM, v_m_s: float = T10_V_M_S) -> np.ndarray:
    """True arrival time at P0..P3 relative to the earliest, in microseconds."""
    d = np.linalg.norm(PIEZO_POS_CM - np.asarray(pos_cm), axis=1)
    t = d / (v_m_s * 1e-4)  # m/s -> cm/us
    return t - t.min()


def piezo_stream(
    name: str,
    knocks: int = 3,
    pre: int = 30,
    post: int = 170,
    seed: int = 10,
    amps=(900.0, 700.0, 500.0, 600.0),
    clip_knock: int | None = None,
    fw_onsets: tuple[int, ...] = (),
    pos_cm=T10_POS_CM,
) -> str:
    """A t09/t10-style stream: full rate (10 us), header repeated per knock."""
    rng = np.random.default_rng(seed)
    arrivals = piezo_arrivals_us(pos_cm)
    header = f"# {name},knock,sample,t_us,ch0,ch1,ch2,ch3"
    lines = []
    for k in range(1, knocks + 1):
        lines.append(header)
        lines.append(f"# meta knock={k},rate_hz=100000,decim=1,pre={pre},skew_ns=2500")
        t = (np.arange(pre + post) - pre) * 10.0
        cols = []
        for ch in range(4):
            # Channel ch is sampled ch * 2.5 us after the row's nominal time;
            # the earliest channel arrives 5 us after the trigger row.
            sample_time = t + ch * SKEW_US
            scale = 4.0 if k == clip_knock else 1.0
            v = 2048.0 + (-1) ** ch * 3.0 + _burst(sample_time, arrivals[ch] + 5.0, amps[ch] * scale)
            v += rng.normal(0.0, 1.0, len(t))
            cols.append(np.clip(np.rint(v), 0, 4095).astype(int))
        for i in range(len(t)):
            lines.append(f"{k},{i},{int(t[i])}," + ",".join(str(c[i]) for c in cols))
        if k in fw_onsets:
            lines.append(f"# knock {k} onsets_us," + ",".join(f"{a + 5.0:.1f}" for a in arrivals))
    lines.append("# end")
    return "\n".join(lines) + "\n"


def t10_long_window(seed: int = 11) -> str:
    """A `long-window` capture: 250 us per row, peak-held decaying envelope."""
    rng = np.random.default_rng(seed)
    pre, post = 8, 240
    lines = [
        "# t10_piezo_time,knock,sample,t_us,ch0,ch1,ch2,ch3",
        f"# meta knock=1,rate_hz=100000,decim=25,pre={pre},skew_ns=2500",
    ]
    t = (np.arange(pre + post) - pre) * 250.0
    tau_us = [6000.0, 8000.0, 10000.0, 7000.0]
    for i, ti in enumerate(t):
        row = []
        for ch in range(4):
            env = 1500.0 * np.exp(-ti / tau_us[ch]) if ti >= 0 else 0.0
            sign = 1 if (i + ch) % 2 == 0 else -1
            row.append(int(np.clip(round(2048 + sign * (env + abs(rng.normal(0, 2.0)))), 0, 4095)))
        lines.append(f"1,{i},{int(ti)}," + ",".join(map(str, row)))
    # Time until the envelope falls below 60 counts: tau * ln(1500 / 60).
    ring = [250.0 * (np.floor(tau * np.log(1500.0 / 60.0) / 250.0) + 1) for tau in tau_us]
    lines.append("# knock 1 ringdown_us," + ",".join(f"{r:.1f}" for r in ring))
    lines.append("# end")
    return "\n".join(lines) + "\n"


SAMPLES = {
    "t06_noise.csv": lambda: t06_noise(),
    "t07_capture.csv": lambda: t07_capture(near=0, knocks=3),
    "t09_piezo_dc.csv": lambda: piezo_stream("t09_piezo_dc", knocks=2, clip_knock=2, seed=9),
    "t10_piezo_time.csv": lambda: piezo_stream("t10_piezo_time", knocks=3, fw_onsets=(1, 2)),
    "t10_piezo_time_long.csv": t10_long_window,
}


def main() -> None:
    DATA_DIR.mkdir(parents=True, exist_ok=True)
    for name, make in SAMPLES.items():
        (DATA_DIR / name).write_text(make(), encoding="utf-8")
        print(f"wrote {DATA_DIR / name}")


if __name__ == "__main__":
    main()
