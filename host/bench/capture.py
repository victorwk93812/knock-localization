#!/usr/bin/env python3
"""Record and analyse the CSV streams of the knock-localization bench tests.

The firmware sends line-oriented text on the ST-LINK virtual COM port:

    # t10_piezo_time,knock,sample,t_us,ch0,ch1,ch2,ch3     <- stream header
    # meta knock=1,rate_hz=100000,decim=1,pre=100,skew_ns=2500
    1,0,-1000,2048,2047,2049,2048                          <- data rows
    # knock 1 onsets_us,12.5,48.0,95.5,51.0                <- comment lines
    # end                                                  <- end of the run

Usage (from host/):

    uv run bench/capture.py --port /dev/ttyACM0 --out data/   # record + analyse
    uv run bench/capture.py --replay data/<file>.csv          # analyse a file
    uv run bench/capture.py --port /dev/ttyACM0 --echo        # t00_board echo test
    uv run bench/capture.py --replay f0.csv --mode gain --sensor 0
    uv run bench/capture.py --mode gain-solve f0.csv ... f8.csv [--center c.csv]
    uv run bench/capture.py --replay t10.csv --known-pos 0,0

Each run is saved verbatim to <out>/<bin>-<timestamp>.csv. The analysis is
printed and saved as PNG beside the CSV (--no-plot to skip, --show for windows).
Start the script before resetting the board so the first header is not missed.

The file is organised as: parsing (pure) / analysis (pure, text + numbers) /
plots / serial I/O / command line. Only the last two touch the outside world.
"""

from __future__ import annotations

import argparse
import math
import re
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterable, Sequence

import numpy as np

# --------------------------------------------------------------------------
# Constants shared with the firmware
# --------------------------------------------------------------------------

DEFAULT_BAUD = 921_600

#: Piezo positions in cm, origin at the plate centre; channel k = Pk.
PIEZO_POS_CM = np.array([[-5.0, 3.33], [5.0, 3.33], [5.0, -3.33], [-5.0, -3.33]])

#: Defaults used when a piezo stream carries no `# meta` line.
PIEZO_RATE_HZ = 100_000.0
PIEZO_SKEW_US = 2.5
ONSET_K_SIGMA = 8.0
ONSET_MIN_COUNTS = 6.0

#: Accelerometer layout: corners, and the pairs that must match for a centre knock.
SENSORS = 9
CORNERS = (0, 2, 4, 6)
SYMMETRY_PAIRS = ((1, 5), (3, 7))

PIEZO_COLUMNS = ["knock", "sample", "t_us", "ch0", "ch1", "ch2", "ch3"]
T07_COLUMNS = ["knock", "sensor", "sample", "x_mg", "y_mg", "z_mg"]

# --------------------------------------------------------------------------
# Parsing
# --------------------------------------------------------------------------

_HEADER_RE = re.compile(r"^#\s*([A-Za-z]\w*)((?:,[A-Za-z_]\w*)+)\s*$")
_END_RE = re.compile(r"^#\s*end\s*$")
_KNOCK_NOTE_RE = re.compile(r"^knock\s+(\d+)\s+(\w+),(.*)$")
_META_RE = re.compile(r"^meta\s+(.*)$")


def _to_float(text: str) -> float:
    return float(text.strip())


@dataclass
class Run:
    """One stream from its header to `# end`."""

    name: str
    columns: list[str]
    rows: list[list[float]] = field(default_factory=list)
    comments: list[str] = field(default_factory=list)
    ended: bool = False
    bad_lines: int = 0

    def array(self) -> np.ndarray:
        """All rows as a float array of shape (n, len(columns))."""
        if not self.rows:
            return np.empty((0, len(self.columns)))
        return np.asarray(self.rows, dtype=float)

    def has_columns(self, names: Iterable[str]) -> bool:
        return all(n in self.columns for n in names)

    def column(self, name: str) -> np.ndarray:
        return self.array()[:, self.columns.index(name)]

    def knock_notes(self, key: str) -> dict[int, list[float]]:
        """Values of the `# knock <n> <key>,<v0>,<v1>,...` lines, by knock."""
        notes: dict[int, list[float]] = {}
        for comment in self.comments:
            m = _KNOCK_NOTE_RE.match(comment)
            if not m or m.group(2) != key:
                continue
            try:
                notes[int(m.group(1))] = [_to_float(v) for v in m.group(3).split(",")]
            except ValueError:
                continue
        return notes

    def meta(self) -> dict[str, float]:
        """Key/value pairs of the last `# meta k=v,k=v` line."""
        result: dict[str, float] = {}
        for comment in self.comments:
            m = _META_RE.match(comment)
            if not m:
                continue
            for pair in m.group(1).split(","):
                key, _, value = pair.partition("=")
                try:
                    result[key.strip()] = float(value)
                except ValueError:
                    pass
        return result


@dataclass
class Event:
    """What one input line meant. `kind` is one of: header, reheader, row,
    comment, end, bad (malformed row inside a run), noise (outside any run)."""

    kind: str
    run: Run | None = None
    text: str = ""
    values: list[float] | None = None


class StreamParser:
    """Incremental parser of the CSV protocol. Feed it one line at a time."""

    def __init__(self) -> None:
        self.current: Run | None = None
        self.runs: list[Run] = []

    def feed(self, line: str) -> Event:
        line = line.strip("\r\n")
        text = line.strip()
        if not text:
            return Event("noise", self.current)

        if text.startswith("#"):
            if _END_RE.match(text):
                run, self.current = self.current, None
                if run is None:
                    return Event("noise", None, text)
                run.ended = True
                return Event("end", run, text)
            m = _HEADER_RE.match(text)
            if m:
                name = m.group(1)
                columns = m.group(2).lstrip(",").split(",")
                run = self.current
                if run is not None and run.name == name and run.columns == columns:
                    # The piezo tests repeat the header before every knock.
                    return Event("reheader", run, text)
                self.current = Run(name, columns)
                self.runs.append(self.current)
                return Event("header", self.current, text)
            if self.current is None:
                return Event("noise", None, text)
            comment = text[1:].strip()
            self.current.comments.append(comment)
            return Event("comment", self.current, comment)

        run = self.current
        if run is None:
            return Event("noise", None, text)
        fields = text.split(",")
        try:
            values = [_to_float(f) for f in fields]
        except ValueError:
            values = None
        if values is None or len(values) != len(run.columns):
            # Typically a line cut in half by a reset or a late attach.
            run.bad_lines += 1
            return Event("bad", run, text)
        run.rows.append(values)
        return Event("row", run, text, values)


def parse_lines(lines: Iterable[str]) -> list[Run]:
    """Parse a whole capture; returns every run found (ended or not)."""
    parser = StreamParser()
    for line in lines:
        parser.feed(line)
    return parser.runs


def parse_text(text: str) -> list[Run]:
    return parse_lines(text.splitlines())


def parse_file(path: Path | str) -> list[Run]:
    with open(path, "r", encoding="utf-8", errors="replace") as fh:
        return parse_lines(fh)


def split_knocks(run: Run) -> dict[int, np.ndarray]:
    """Rows grouped by the `knock` column, in order of first appearance."""
    data = run.array()
    if data.shape[0] == 0 or "knock" not in run.columns:
        return {}
    knock = data[:, run.columns.index("knock")].astype(int)
    return {int(k): data[knock == k] for k in dict.fromkeys(knock.tolist())}


# --------------------------------------------------------------------------
# Analysis helpers
# --------------------------------------------------------------------------


@dataclass
class Options:
    """Analysis options (from the command line)."""

    mode: str = "auto"
    sensor: int | None = None
    known_pos: tuple[float, float] | None = None
    pre_samples: int = 4
    max_plots: int = 12


@dataclass
class Analysis:
    """Result of analysing one run: text for the terminal, numbers for tests,
    and an optional function that draws the plots."""

    text: str
    data: dict = field(default_factory=dict)
    plot: Callable[[Path, bool], list[Path]] | None = None


def _fmt(value: float, spec: str = "8.1f") -> str:
    if value is None or (isinstance(value, float) and math.isnan(value)):
        return format("nan", ">" + re.sub(r"\..*", "", spec))
    if isinstance(value, float) and math.isinf(value):
        return format("inf", ">" + re.sub(r"\..*", "", spec))
    return format(value, spec)


def table(headers: Sequence[str], rows: Sequence[Sequence[str]]) -> str:
    widths = [len(h) for h in headers]
    for row in rows:
        widths = [max(w, len(str(c))) for w, c in zip(widths, row)]
    lines = ["  ".join(str(h).rjust(w) for h, w in zip(headers, widths))]
    lines.append("  ".join("-" * w for w in widths))
    lines += ["  ".join(str(c).rjust(w) for c, w in zip(row, widths)) for row in rows]
    return "\n".join(lines)


# --------------------------------------------------------------------------
# t06_noise
# --------------------------------------------------------------------------


def _axis_columns(run: Run) -> list[str]:
    for candidate in (["x_mg", "y_mg", "z_mg"], ["x", "y", "z"]):
        if run.has_columns(candidate):
            return candidate
    return []


def noise_rms(run: Run) -> dict[int, np.ndarray]:
    """RMS deviation from the mean per sensor: {sensor: [rms_x, rms_y, rms_z]}."""
    axes = _axis_columns(run)
    if not axes or "sensor" not in run.columns:
        raise ValueError(
            f"{run.name}: need a 'sensor' column and x/y/z (or x_mg/y_mg/z_mg); "
            f"got {','.join(run.columns)}"
        )
    data = run.array()
    sensor = data[:, run.columns.index("sensor")].astype(int)
    idx = [run.columns.index(a) for a in axes]
    return {
        int(s): data[sensor == s][:, idx].std(axis=0) for s in sorted(set(sensor.tolist()))
    }


def analyze_t06(run: Run, opts: Options) -> Analysis:
    rms = noise_rms(run)
    axes = _axis_columns(run)
    unit = "mg" if axes[0].endswith("_mg") else "raw"
    counts = {s: int((run.column("sensor").astype(int) == s).sum()) for s in rms}
    rows = [
        [f"I{s}", counts[s]] + [f"{v:.2f}" for v in r] + [f"{5 * float(r.max()):.2f}"]
        for s, r in rms.items()
    ]
    text = (
        f"{run.name}: RMS noise per sensor and axis [{unit}] "
        f"(deviation from each axis mean)\n"
        + table(["sensor", "samples", "rms_x", "rms_y", "rms_z", "5*sigma_max"], rows)
        + "\nT6 asks for detection thresholds of at least 5 sigma (last column)."
    )
    return Analysis(text, {"rms": rms, "unit": unit}, lambda p, show: plot_t06(rms, unit, p, show))


# --------------------------------------------------------------------------
# t07_capture
# --------------------------------------------------------------------------


def knock_peaks(run: Run, pre_samples: int = 4) -> dict[int, np.ndarray]:
    """Peak dynamic acceleration per sensor for every knock: {knock: A[9]}.

    Uses the firmware's `# knock <n> peaks,...` lines when present. Otherwise
    A_i = max_t |a_i(t) - g_i| with g_i the mean of the first `pre_samples`
    samples of sensor i in that knock's window (the pre-trigger part).
    """
    noted = run.knock_notes("peaks")
    if not run.has_columns(T07_COLUMNS):
        if noted:
            return {k: np.asarray(v, dtype=float) for k, v in noted.items()}
        raise ValueError(f"{run.name}: expected columns {','.join(T07_COLUMNS)}")
    peaks: dict[int, np.ndarray] = {}
    ci = {c: run.columns.index(c) for c in T07_COLUMNS}
    for knock, rows in split_knocks(run).items():
        if knock in noted:
            peaks[knock] = np.asarray(noted[knock], dtype=float)
            continue
        sensors = rows[:, ci["sensor"]].astype(int)
        size = max(SENSORS, int(sensors.max()) + 1)
        a = np.full(size, np.nan)
        for s in sorted(set(sensors.tolist())):
            block = rows[sensors == s]
            block = block[np.argsort(block[:, ci["sample"]], kind="stable")]
            xyz = block[:, [ci["x_mg"], ci["y_mg"], ci["z_mg"]]]
            base = xyz[: max(1, min(pre_samples, len(xyz)))].mean(axis=0)
            a[s] = float(np.linalg.norm(xyz - base, axis=1).max())
        peaks[knock] = a
    for knock, values in noted.items():
        peaks.setdefault(knock, np.asarray(values, dtype=float))
    return dict(sorted(peaks.items()))


def _peak_matrix(peaks: dict[int, np.ndarray]) -> np.ndarray:
    if not peaks:
        return np.empty((0, SENSORS))
    width = max(len(v) for v in peaks.values())
    out = np.full((len(peaks), width), np.nan)
    for i, v in enumerate(peaks.values()):
        out[i, : len(v)] = v
    return out


def gain_stats(peaks: dict[int, np.ndarray], sensor: int) -> dict:
    """Statistics of "knocks near sensor K"."""
    m = _peak_matrix(peaks)
    if m.shape[0] == 0 or sensor >= m.shape[1]:
        raise ValueError(f"no peaks for sensor {sensor}")
    own = m[:, sensor]
    mean = float(np.nanmean(own))
    std = float(np.nanstd(own))
    means = np.nanmean(m, axis=0)
    return {
        "sensor": sensor,
        "knocks": int(m.shape[0]),
        "mean": mean,
        "std": std,
        "cv": std / mean if mean else float("nan"),
        "means": means,
        "loudest": int(np.nanargmax(means)),
    }


def analyze_t07(run: Run, opts: Options) -> Analysis:
    peaks = knock_peaks(run, opts.pre_samples)
    m = _peak_matrix(peaks)
    headers = ["knock"] + [f"I{i}" for i in range(m.shape[1])] + ["max at"]
    rows = [
        [k] + [_fmt(float(v), ".0f") for v in a] + [f"I{int(np.nanargmax(a))}"]
        for k, a in zip(peaks, m)
    ]
    text = f"{run.name}: peak dynamic acceleration per sensor [mg], {len(peaks)} knock(s)\n"
    text += table(headers, rows) if rows else "(no knocks)"
    data: dict = {"peaks": peaks}
    if opts.mode == "gain":
        if opts.sensor is None:
            raise ValueError("--mode gain needs --sensor K")
        g = gain_stats(peaks, opts.sensor)
        data["gain"] = g
        text += (
            f"\n\n{g['knocks']} knocks near sensor I{g['sensor']}: "
            f"A_{g['sensor']} mean {g['mean']:.1f} mg, std {g['std']:.1f} mg, "
            f"CV {100 * g['cv']:.1f} %"
        )
        if g["knocks"] != 20:
            text += f"\n  note: T7 asks for 20 knocks per sensor, this file has {g['knocks']}"
        if g["loudest"] != g["sensor"]:
            text += (
                f"\n  WARNING: the largest mean response is on I{g['loudest']}, "
                f"not I{g['sensor']}: wrong --sensor, wrong knock position, or swapped channels?"
            )
        text += "\n  mean response of every sensor [mg]: " + " ".join(
            f"I{i}={v:.0f}" for i, v in enumerate(g["means"])
        )
    return Analysis(text, data, lambda p, show: plot_t07(m, opts.sensor, p, show))


def solve_gains(responses: Sequence[float]) -> np.ndarray:
    """Gains c_i that equalise the responses R_i, normalised to mean 1.

    R_i is the mean peak of sensor i for knocks next to sensor i. After
    calibration c_i * R_i is the same for every sensor.
    """
    r = np.asarray(responses, dtype=float)
    if np.any(~np.isfinite(r)) or np.any(r <= 0):
        raise ValueError("every response must be positive and finite")
    c = 1.0 / r
    return c / c.mean()


def symmetry_report(values: np.ndarray, label: str) -> tuple[str, dict]:
    """Ratios of the pairs that must be equal by symmetry, and corner spread."""
    lines = []
    data: dict = {}
    for a, b in SYMMETRY_PAIRS:
        ratio = float(values[a] / values[b])
        data[f"I{a}/I{b}"] = ratio
        lines.append(f"  {label} I{a}/I{b} = {ratio:.3f}")
    corners = values[list(CORNERS)]
    spread = float((corners.max() - corners.min()) / corners.mean())
    data["corner_spread"] = spread
    lines.append(
        f"  {label} corners I0/I2/I4/I6 = "
        + " ".join(f"{v:.3f}" for v in corners)
        + f"  spread (max-min)/mean = {100 * spread:.1f} %"
    )
    return "\n".join(lines), data


def analyze_gain_solve(
    runs: Sequence[Run], opts: Options, center: Run | None = None
) -> Analysis:
    """Combine nine "knocks near sensor k" runs (in sensor order) into gains."""
    if len(runs) != SENSORS:
        raise ValueError(f"gain-solve needs {SENSORS} files (sensor 0..8 in order), got {len(runs)}")
    stats = [gain_stats(knock_peaks(run, opts.pre_samples), k) for k, run in enumerate(runs)]
    responses = np.array([s["mean"] for s in stats])
    gains = solve_gains(responses)
    rows = [
        [f"I{k}", s["knocks"], f"{s['mean']:.1f}", f"{100 * s['cv']:.1f}", f"{g:.3f}",
         "" if s["loudest"] == k else f"loudest is I{s['loudest']}!"]
        for k, (s, g) in enumerate(zip(stats, gains))
    ]
    raw_spread = float((responses.max() - responses.min()) / responses.mean())
    text = "gain-solve: file k = knocks near sensor k\n"
    text += table(["sensor", "knocks", "R_i [mg]", "CV [%]", "c_i", "note"], rows)
    text += (
        f"\nspread of the uncalibrated responses (max-min)/mean = {100 * raw_spread:.1f} %"
        f"\nlargest knock-to-knock CV = {100 * max(s['cv'] for s in stats):.1f} % "
        "(the calibration cannot be better than the repeatability of the knocks)"
        "\nc_i * R_i is equal for all sensors by construction; the T7 criterion "
        "(spread < 10 % after calibration) is checked on centre knocks (--center)."
    )
    sym_text, sym = symmetry_report(gains, "gain")
    text += "\nsymmetry of the gains themselves (1.000 = equal sensitivity):\n" + sym_text
    data = {"gains": gains, "responses": responses, "raw_spread": raw_spread, "gain_symmetry": sym}
    if center is not None:
        m = _peak_matrix(knock_peaks(center, opts.pre_samples))
        calibrated = np.nanmean(m, axis=0)[:SENSORS] * gains
        c_text, c_sym = symmetry_report(calibrated, "calibrated centre response")
        ok = (
            all(abs(c_sym[f"I{a}/I{b}"] - 1) < 0.10 for a, b in SYMMETRY_PAIRS)
            and c_sym["corner_spread"] < 0.10
        )
        text += (
            f"\ncentre knocks ({m.shape[0]}) after calibration:\n{c_text}\n"
            f"  T7 (spread < 10 %): {'PASS' if ok else 'FAIL'}"
        )
        data["center_symmetry"] = c_sym
        data["center_pass"] = ok
    return Analysis(text, data, lambda p, show: plot_gains(gains, p, show))


# --------------------------------------------------------------------------
# t09_piezo_dc / t10_piezo_time
# --------------------------------------------------------------------------


@dataclass
class PiezoKnock:
    knock: int
    t_us: np.ndarray  # (n,)
    counts: np.ndarray  # (n, 4)

    @property
    def pre(self) -> int:
        """Number of samples before the trigger (t < 0)."""
        n = int((self.t_us < 0).sum())
        return n if n > 0 else max(1, len(self.t_us) // 10)

    def baseline(self) -> np.ndarray:
        return self.counts[: self.pre].mean(axis=0)

    def peak_dev(self) -> np.ndarray:
        return np.abs(self.counts - self.baseline()).max(axis=0)


def piezo_knocks(run: Run) -> list[PiezoKnock]:
    if not run.has_columns(PIEZO_COLUMNS):
        raise ValueError(f"{run.name}: expected columns {','.join(PIEZO_COLUMNS)}")
    ci = [run.columns.index(c) for c in PIEZO_COLUMNS]
    knocks = []
    for knock, rows in split_knocks(run).items():
        rows = rows[np.argsort(rows[:, ci[1]], kind="stable")]
        knocks.append(PiezoKnock(knock, rows[:, ci[2]], rows[:, ci[3:]]))
    return knocks


def onset_position(
    samples: np.ndarray, baseline_len: int, k_sigma: float, min_threshold: float
) -> float:
    """Fractional sample index of the first crossing of max(k*sigma, min) over
    the baseline mean, linearly interpolated; NaN if none. Same rule as the
    firmware (`knock_drivers::piezo::onset`)."""
    if baseline_len <= 0 or baseline_len >= len(samples):
        return float("nan")
    base = samples[:baseline_len]
    dev = np.abs(samples - base.mean())
    threshold = max(k_sigma * float(base.std()), min_threshold)
    above = np.nonzero(dev[baseline_len:] > threshold)[0]
    if len(above) == 0:
        return float("nan")
    i = baseline_len + int(above[0])
    before, after = float(dev[i - 1]), float(dev[i])
    fraction = (threshold - before) / (after - before) if before < threshold else 1.0
    return i - 1 + fraction


def host_onsets_us(knock: PiezoKnock, skew_us: float = PIEZO_SKEW_US) -> np.ndarray:
    """Onset time per channel in microseconds relative to the trigger, with
    the scan skew applied (channel k is sampled k*skew after channel 0)."""
    pre = knock.pre
    step = float(np.median(np.diff(knock.t_us))) if len(knock.t_us) > 1 else 10.0
    out = np.full(4, np.nan)
    for ch in range(4):
        pos = onset_position(knock.counts[:, ch], pre, ONSET_K_SIGMA, ONSET_MIN_COUNTS)
        if not math.isnan(pos):
            out[ch] = float(knock.t_us[0]) + pos * step + ch * skew_us
    return out


def estimate_v_eff(onsets_us: np.ndarray, pos_cm: tuple[float, float]) -> dict:
    """Effective wave speed from the arrival times of one knock at a known
    position: least-squares line t_i = t0 + d_i / v over the four piezos."""
    d = np.linalg.norm(PIEZO_POS_CM - np.asarray(pos_cm, dtype=float), axis=1)
    ok = np.isfinite(onsets_us)
    result = {"distances_cm": d, "v_m_s": float("nan"), "reason": ""}
    if ok.sum() < 3:
        result["reason"] = "fewer than 3 onsets"
        return result
    if np.ptp(d[ok]) < 0.5:
        result["reason"] = "all piezos (nearly) equidistant: speed not observable here"
        return result
    slope, intercept = np.polyfit(d[ok], onsets_us[ok], 1)  # us per cm
    if slope <= 0:
        result["reason"] = "arrival order contradicts the distances"
        return result
    resid = onsets_us[ok] - (slope * d[ok] + intercept)
    result["v_m_s"] = 1e4 / slope  # cm/us -> m/s
    result["residual_us"] = float(np.sqrt(np.mean(resid**2)))
    return result


def analyze_piezo(run: Run, opts: Options) -> Analysis:
    knocks = piezo_knocks(run)
    meta = run.meta()
    skew_us = meta.get("skew_ns", PIEZO_SKEW_US * 1000) / 1000
    decim = int(meta.get("decim", 1))
    text = f"{run.name}: {len(knocks)} knock(s)"
    if decim > 1:
        text += f", decimated by {decim} with peak hold"
    rows = []
    for k in knocks:
        base, peak = k.baseline(), k.peak_dev()
        rails = [(k.counts[:, c].min() <= 0) or (k.counts[:, c].max() >= 4095) for c in range(4)]
        rows.append(
            [k.knock, len(k.t_us)]
            + [f"{b:.0f}" for b in base]
            + [f"{p:.0f}" + ("*" if r else "") for p, r in zip(peak, rails)]
        )
    text += "\n" + table(
        ["knock", "rows"] + [f"base{c}" for c in range(4)] + [f"peak{c}" for c in range(4)], rows
    )
    text += "\n(counts; peak = largest deviation from the pre-trigger mean; * = a sample on an ADC rail)"
    data: dict = {"knocks": knocks}
    onsets: dict[int, np.ndarray] = {}

    if run.name.startswith("t10") or opts.known_pos is not None:
        fw = run.knock_notes("onsets_us")
        ring = run.knock_notes("ringdown_us")
        if decim == 1:
            orows = []
            for k in knocks:
                source = "fw" if k.knock in fw and len(fw[k.knock]) == 4 else "host"
                t = np.asarray(fw[k.knock]) if source == "fw" else host_onsets_us(k, skew_us)
                onsets[k.knock] = t
                if np.isfinite(t).any():
                    first = int(np.nanargmin(t))
                    dt = t - t[first]
                    order = " ".join(f"P{c}" for c in np.argsort(t) if np.isfinite(t[c]))
                else:
                    dt, order = t, "-"
                orows.append(
                    [k.knock, source]
                    + [_fmt(float(v), ".1f") for v in t]
                    + [_fmt(float(v), ".1f") for v in dt]
                    + [order]
                )
            text += "\n\nonset times [us] relative to the trigger, and relative to the earliest channel\n"
            text += table(
                ["knock", "src"] + [f"t{c}" for c in range(4)] + [f"dt{c}" for c in range(4)] + ["order"],
                orows,
            )
            data["onsets_us"] = onsets
        if ring:
            rrows = [[k] + [_fmt(float(v) / 1000, ".1f") for v in vals] for k, vals in ring.items()]
            finite = [v for vals in ring.values() for v in vals if math.isfinite(v)]
            text += "\n\nring-down [ms] per channel (nan = never above threshold, inf = still ringing)\n"
            text += table(["knock"] + [f"ch{c}" for c in range(4)], rrows)
            if finite:
                text += f"\nlongest ring-down {max(finite) / 1000:.1f} ms: refractory period must be longer"
            data["ringdown_us"] = ring

    if opts.known_pos is not None and onsets:
        estimates = {k: estimate_v_eff(t, opts.known_pos) for k, t in onsets.items()}
        speeds = [e["v_m_s"] for e in estimates.values() if math.isfinite(e["v_m_s"])]
        d = next(iter(estimates.values()))["distances_cm"]
        text += (
            f"\n\nv_eff for knocks at ({opts.known_pos[0]:g}, {opts.known_pos[1]:g}) cm; "
            "distances to P0..P3 [cm]: " + " ".join(f"{x:.2f}" for x in d)
        )
        for k, e in estimates.items():
            if math.isfinite(e["v_m_s"]):
                text += f"\n  knock {k}: {e['v_m_s']:.0f} m/s (fit residual {e['residual_us']:.1f} us)"
            else:
                text += f"\n  knock {k}: no estimate ({e['reason']})"
        if speeds:
            text += f"\n  median v_eff = {float(np.median(speeds)):.0f} m/s over {len(speeds)} knock(s)"
            data["v_eff_m_s"] = float(np.median(speeds))
        else:
            spread = [float(np.nanmax(t) - np.nanmin(t)) for t in onsets.values() if np.isfinite(t).any()]
            if spread:
                text += (
                    f"\n  arrival spread (latest - earliest): median {float(np.median(spread)):.1f} us "
                    "(expected near 0 for a centre knock)"
                )
        data["v_estimates"] = estimates

    def plot(csv_path: Path, show: bool) -> list[Path]:
        return plot_piezo(run.name, knocks, onsets, csv_path, show, opts.max_plots)

    return Analysis(text, data, plot)


# --------------------------------------------------------------------------
# Dispatch
# --------------------------------------------------------------------------


def analyze(run: Run, opts: Options) -> Analysis:
    """Pick the analysis by stream name."""
    if not run.rows and not run.comments:
        return Analysis(f"{run.name}: empty run")
    if run.name.startswith("t06"):
        return analyze_t06(run, opts)
    if run.name.startswith("t07"):
        return analyze_t07(run, opts)
    if run.name.startswith(("t09", "t10")):
        return analyze_piezo(run, opts)
    return Analysis(
        f"{run.name}: no analysis for this stream; {len(run.rows)} rows, "
        f"columns {','.join(run.columns)}"
    )


def knock_summary(run: Run, knock: int, opts: Options) -> str:
    """One line for the live display when a knock is complete."""
    rows = split_knocks(run).get(knock)
    if rows is None:
        return f"{run.name} knock {knock}: no rows"
    if run.name.startswith("t07") and run.has_columns(T07_COLUMNS):
        single = Run(run.name, run.columns, rows.tolist(), run.comments)
        a = knock_peaks(single, opts.pre_samples).get(knock)
        if a is None:
            return f"{run.name} knock {knock}: {len(rows)} rows"
        return (
            f"{run.name} knock {knock}: peaks [mg] "
            + " ".join(_fmt(float(v), ".0f") for v in a)
            + f"  max at I{int(np.nanargmax(a))}"
        )
    if run.has_columns(PIEZO_COLUMNS):
        single = Run(run.name, run.columns, rows.tolist(), run.comments)
        k = piezo_knocks(single)[0]
        line = f"{run.name} knock {knock}: {len(k.t_us)} rows, peak dev [counts] " + " ".join(
            f"{p:.0f}" for p in k.peak_dev()
        )
        fw = run.knock_notes("onsets_us").get(knock)
        if fw and np.isfinite(fw).any():
            line += f", first P{int(np.nanargmin(fw))}"
        return line
    return f"{run.name} knock {knock}: {len(rows)} rows"


# --------------------------------------------------------------------------
# Plots (matplotlib is imported only here)
# --------------------------------------------------------------------------

# Fixed categorical order; identity is also carried by panel titles / legends.
SERIES = ["#2a78d6", "#eb6834", "#1baf7a", "#eda100"]
INK, INK_2, MUTED = "#0b0b0b", "#52514e", "#898781"
SURFACE, GRID, AXIS = "#fcfcfb", "#e1e0d9", "#c3c2b7"


def _pyplot(show: bool):
    import matplotlib

    if not show:
        matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    plt.rcParams.update(
        {
            "figure.facecolor": SURFACE,
            "axes.facecolor": SURFACE,
            "savefig.facecolor": SURFACE,
            "axes.edgecolor": AXIS,
            "axes.labelcolor": INK_2,
            "axes.titlecolor": INK,
            "text.color": INK,
            "xtick.color": MUTED,
            "ytick.color": MUTED,
            "xtick.labelcolor": INK_2,
            "ytick.labelcolor": INK_2,
            "grid.color": GRID,
            "grid.linewidth": 0.6,
            "axes.grid": True,
            "axes.axisbelow": True,
            "axes.spines.top": False,
            "axes.spines.right": False,
            "axes.titlesize": 10,
            "axes.titleweight": "bold",
            "axes.titlelocation": "left",
            "axes.labelsize": 9,
            "xtick.labelsize": 8,
            "ytick.labelsize": 8,
            "legend.frameon": False,
            "legend.fontsize": 8,
            "lines.linewidth": 1.4,
            "font.size": 9,
        }
    )
    return plt


def _finish(plt, fig, path: Path, show: bool) -> Path:
    fig.savefig(path, dpi=130)
    if show:
        plt.show()
    plt.close(fig)
    return path


def plot_piezo(
    name: str,
    knocks: list[PiezoKnock],
    onsets: dict[int, np.ndarray],
    csv_path: Path,
    show: bool,
    max_plots: int,
) -> list[Path]:
    """One PNG per knock: four stacked panels (one per channel, shared time
    axis), so the traces never overlap and need no colour to tell apart."""
    plt = _pyplot(show)
    paths = []
    for k in knocks[:max_plots]:
        fig, axes = plt.subplots(4, 1, sharex=True, figsize=(8, 7), layout="constrained")
        fig.suptitle(f"{name}, knock {k.knock}", x=0.02, ha="left", fontsize=11, fontweight="bold")
        t_ms = k.t_us / 1000.0
        base = k.baseline()
        t_on = onsets.get(k.knock)
        for ch, ax in enumerate(axes):
            ax.plot(t_ms, k.counts[:, ch], color=SERIES[ch])
            ax.axhline(base[ch], color=AXIS, linewidth=0.8)
            title = f"ch{ch} (P{ch})   peak {k.peak_dev()[ch]:.0f} counts"
            if t_on is not None and np.isfinite(t_on[ch]):
                ax.axvline(t_on[ch] / 1000.0, color=INK_2, linewidth=0.9, linestyle="--")
                title += f"   onset {t_on[ch]:.1f} us"
            ax.set_title(title)
            ax.set_ylabel("counts")
        axes[-1].set_xlabel("time relative to the trigger [ms]  (dashed: onset)")
        paths.append(_finish(plt, fig, csv_path.with_name(f"{csv_path.stem}-knock{k.knock}.png"), show))
    return paths


def plot_t06(rms: dict[int, np.ndarray], unit: str, csv_path: Path, show: bool) -> list[Path]:
    plt = _pyplot(show)
    fig, ax = plt.subplots(figsize=(8, 4), layout="constrained")
    sensors = list(rms)
    x = np.arange(len(sensors))
    for i, axis in enumerate("xyz"):
        ax.bar(x + (i - 1) * 0.27, [rms[s][i] for s in sensors], width=0.24, color=SERIES[i], label=f"{axis} axis")
    ax.set_xticks(x, [f"I{s}" for s in sensors])
    ax.grid(axis="x", visible=False)
    ax.set_ylabel(f"RMS noise [{unit}]")
    ax.set_title("Noise floor per sensor and axis")
    ax.legend(ncols=3, loc="lower right", bbox_to_anchor=(1.0, 1.0), borderaxespad=0.2)
    return [_finish(plt, fig, csv_path.with_suffix(".png"), show)]


def plot_t07(m: np.ndarray, sensor: int | None, csv_path: Path, show: bool) -> list[Path]:
    if m.shape[0] == 0:
        return []
    plt = _pyplot(show)
    fig, ax = plt.subplots(figsize=(8, 4), layout="constrained")
    x = np.arange(m.shape[1])
    mean, std = np.nanmean(m, axis=0), np.nanstd(m, axis=0)
    ax.bar(x, mean, width=0.6, color=SERIES[0], yerr=std, ecolor=INK_2, capsize=3)
    for xi, v in zip(x, mean):
        ax.annotate(f"{v:.0f}", (xi, v), xytext=(0, 3), textcoords="offset points", ha="center", color=INK_2, fontsize=8)
    ax.set_xticks(x, [f"I{i}" for i in x])
    ax.grid(axis="x", visible=False)
    ax.set_ylabel("peak dynamic acceleration [mg]")
    where = f" near I{sensor}" if sensor is not None else ""
    ax.set_title(f"Mean peak per sensor over {m.shape[0]} knock(s){where} (bars: std)")
    return [_finish(plt, fig, csv_path.with_suffix(".png"), show)]


def plot_gains(gains: np.ndarray, csv_path: Path, show: bool) -> list[Path]:
    plt = _pyplot(show)
    fig, ax = plt.subplots(figsize=(8, 4), layout="constrained")
    x = np.arange(len(gains))
    ax.bar(x, gains, width=0.6, color=SERIES[0])
    ax.axhline(1.0, color=AXIS, linewidth=0.8)
    for xi, v in zip(x, gains):
        ax.annotate(f"{v:.3f}", (xi, v), xytext=(0, 3), textcoords="offset points", ha="center", color=INK_2, fontsize=8)
    ax.set_xticks(x, [f"I{i}" for i in x])
    ax.grid(axis="x", visible=False)
    ax.set_ylabel("gain c_i (mean = 1)")
    ax.set_title("Calibration gains")
    return [_finish(plt, fig, csv_path.with_name("gain-solve.png"), show)]


# --------------------------------------------------------------------------
# Reporting and live session (file output, no serial port needed)
# --------------------------------------------------------------------------


def report_run(
    run: Run, opts: Options, csv_path: Path | None, plot: bool, show: bool, out=None
) -> Analysis | None:
    """Print the analysis of one run and save its plots beside `csv_path`."""
    out = out if out is not None else sys.stdout
    try:
        analysis = analyze(run, opts)
    except ValueError as err:
        print(f"analysis failed: {err}", file=out)
        return None
    print(analysis.text, file=out)
    if run.bad_lines:
        print(f"note: {run.bad_lines} malformed line(s) skipped", file=out)
    if not run.ended:
        print("note: the run has no '# end' line (interrupted or still running)", file=out)
    if plot and analysis.plot is not None and csv_path is not None and run.rows:
        for path in analysis.plot(csv_path, show):
            print(f"plot: {path}", file=out)
    return analysis


class LiveSession:
    """Turns incoming lines into files, live one-line summaries and, at the end
    of each run, the analysis. The serial loop only calls `feed_line`/`idle`."""

    def __init__(
        self,
        out_dir: Path,
        opts: Options,
        plot: bool = True,
        show: bool = False,
        out=None,
        timestamp: Callable[[], str] = lambda: time.strftime("%Y%m%d-%H%M%S"),
    ) -> None:
        self.out_dir = Path(out_dir)
        self.opts = opts
        self.plot = plot
        self.show = show
        self.out = out if out is not None else sys.stdout
        self.timestamp = timestamp
        self.parser = StreamParser()
        self.file = None
        self.path: Path | None = None
        self.pending_knock: int | None = None
        self.saved: list[Path] = []

    def _open(self, run: Run) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        path = self.out_dir / f"{run.name}-{self.timestamp()}.csv"
        n = 1
        while path.exists():
            n += 1
            path = self.out_dir / f"{run.name}-{self.timestamp()}-{n}.csv"
        self.path = path
        self.file = open(path, "w", encoding="utf-8", newline="\n")
        self.saved.append(path)
        print(f"recording {run.name} -> {path}", file=self.out)

    def _flush_knock(self, run: Run | None) -> None:
        if run is not None and self.pending_knock is not None:
            try:
                print(knock_summary(run, self.pending_knock, self.opts), file=self.out)
            except (ValueError, IndexError) as err:
                print(f"knock {self.pending_knock}: summary failed ({err})", file=self.out)
        self.pending_knock = None

    def _close(self, run: Run) -> None:
        self._flush_knock(run)
        if self.file is not None:
            self.file.close()
            self.file = None
        report_run(run, self.opts, self.path, self.plot, self.show, self.out)
        self.path = None

    def feed_line(self, line: str) -> None:
        previous = self.parser.current
        event = self.parser.feed(line)
        if event.kind == "header":
            if previous is not None:
                self._close(previous)  # a new stream started without '# end'
            self._open(event.run)
        if event.kind != "noise" and self.file is not None:
            self.file.write(line.rstrip("\r\n") + "\n")
        if event.kind == "row" and "knock" in event.run.columns:
            knock = int(event.values[event.run.columns.index("knock")])
            if knock != self.pending_knock:
                self._flush_knock(event.run)
                self.pending_knock = knock
        elif event.kind == "end":
            self._close(event.run)

    def idle(self) -> None:
        """Call when the line has been quiet: the current knock is complete."""
        if self.file is not None:
            self.file.flush()
        self._flush_knock(self.parser.current)

    def finish(self) -> None:
        """Call on Ctrl-C: close and analyse an unfinished run."""
        if self.parser.current is not None:
            run, self.parser.current = self.parser.current, None
            self._close(run)


# --------------------------------------------------------------------------
# Serial I/O
# --------------------------------------------------------------------------


def _open_port(port: str, baud: int, timeout: float):
    import serial  # pyserial; imported here so --replay works without a port

    return serial.Serial(port, baud, timeout=timeout)


def record(port: str, baud: int, session: LiveSession) -> None:
    """Read lines from the port until Ctrl-C."""
    print(f"listening on {port} at {baud} baud; Ctrl-C to stop", file=session.out)
    with _open_port(port, baud, 0.5) as ser:
        try:
            while True:
                raw = ser.readline()
                if not raw:
                    session.idle()
                    continue
                session.feed_line(raw.decode("utf-8", errors="replace"))
        except KeyboardInterrupt:
            print("\nstopped", file=session.out)
        finally:
            session.finish()


def echo_test(port: str, baud: int) -> None:
    """Interactive echo test for t00_board: every typed line must come back."""
    print(f"echo test on {port} at {baud} baud; type a line + Enter, Ctrl-D to quit")
    with _open_port(port, baud, 0.3) as ser:
        banner = ser.read(4096)
        if banner:
            print("received:", banner.decode("utf-8", errors="replace").strip())
        for line in sys.stdin:
            payload = line.rstrip("\n").encode("utf-8") + b"\r\n"
            ser.reset_input_buffer()
            ser.write(payload)
            ser.flush()
            got = b""
            deadline = time.monotonic() + 1.0
            while len(got) < len(payload) and time.monotonic() < deadline:
                got += ser.read(len(payload) - len(got))
            status = "OK" if got == payload else "MISMATCH"
            print(f"{status}: sent {payload!r}, received {got!r}")


# --------------------------------------------------------------------------
# Command line
# --------------------------------------------------------------------------


def _parse_pos(text: str) -> tuple[float, float]:
    try:
        x, y = (float(v) for v in text.split(","))
    except ValueError:
        raise argparse.ArgumentTypeError("expected X,Y in cm, e.g. 0,0 or -5,3.33")
    return x, y


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="Record and analyse the knock-localization bench-test CSV streams.",
        epilog=(
            "For a position with negative x write --known-pos=-5,3.33 "
            "(with '=') so it is not read as an option."
        ),
    )
    p.add_argument("--port", help="serial port of the ST-LINK virtual COM port, e.g. /dev/ttyACM0")
    p.add_argument("--baud", type=int, default=DEFAULT_BAUD)
    p.add_argument("--out", type=Path, default=Path("data"), help="directory for recorded CSV files")
    p.add_argument("--echo", action="store_true", help="interactive echo test for t00_board")
    p.add_argument("--replay", type=Path, metavar="FILE", help="analyse a saved CSV instead of a port")
    p.add_argument("--no-plot", action="store_true", help="do not write PNG files")
    p.add_argument("--show", action="store_true", help="also open the plots in windows")
    p.add_argument("--mode", choices=["auto", "gain", "gain-solve"], default="auto")
    p.add_argument("--sensor", type=int, metavar="K", help="sensor the knocks were next to (--mode gain)")
    p.add_argument("--center", type=Path, metavar="FILE", help="centre-knock t07 file for the symmetry check (gain-solve)")
    p.add_argument("--known-pos", type=_parse_pos, metavar="X,Y", help="knock position in cm for the v_eff estimate (t10)")
    p.add_argument("--pre-samples", type=int, default=4, help="pre-trigger samples per sensor in a t07 window (default 4 = 10 ms at 400 Hz)")
    p.add_argument("--max-plots", type=int, default=12, help="at most this many per-knock waveform plots per run")
    p.add_argument("files", nargs="*", type=Path, help="nine t07 files, sensor 0..8 in order (--mode gain-solve)")
    return p


def _load_single(path: Path) -> Run:
    runs = [r for r in parse_file(path) if r.rows or r.comments]
    if not runs:
        raise ValueError(f"{path}: no stream header found (expected '# <bin>,<columns>')")
    return runs[0]


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    opts = Options(args.mode, args.sensor, args.known_pos, args.pre_samples, args.max_plots)
    plot = not args.no_plot

    try:
        if args.mode == "gain-solve":
            if not args.files:
                raise ValueError("--mode gain-solve needs nine t07 files (sensor 0..8 in order)")
            runs = [_load_single(f) for f in args.files]
            center = _load_single(args.center) if args.center else None
            analysis = analyze_gain_solve(runs, opts, center)
            print(analysis.text)
            if plot and analysis.plot is not None:
                for path in analysis.plot(args.files[0], args.show):
                    print(f"plot: {path}")
            return 0
        if args.files:
            raise ValueError("positional files are only used with --mode gain-solve")
        if args.replay:
            runs = parse_file(args.replay)
            if not runs:
                raise ValueError(f"{args.replay}: no stream header found (expected '# <bin>,<columns>')")
            ok = True
            for i, run in enumerate(runs):
                target = args.replay if len(runs) == 1 else args.replay.with_name(f"{args.replay.stem}-run{i + 1}.csv")
                ok = report_run(run, opts, target, plot, args.show) is not None and ok
            return 0 if ok else 1
        if not args.port:
            raise ValueError("give --port, or --replay FILE, or --mode gain-solve FILES")
        if args.echo:
            echo_test(args.port, args.baud)
            return 0
        record(args.port, args.baud, LiveSession(args.out, opts, plot, args.show))
        return 0
    except (ValueError, OSError) as err:
        print(f"error: {err}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
