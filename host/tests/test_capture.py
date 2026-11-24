"""Tests for bench/capture.py: the parser and every analysis, on synthetic data
only (no serial port, no hardware)."""

from __future__ import annotations

import io
import math
from pathlib import Path

import numpy as np
import pytest

import capture
import make_data

DATA = Path(__file__).parent / "data"


# ---------------------------------------------------------------- parser


def test_parser_basic_stream():
    runs = capture.parse_text(
        "boot banner, ignored\n"
        "# t05,t_ms,sensor,x\n"
        "12,0,-3\n"
        "14,1,5.5\n"
        "# a comment\n"
        "# end\n"
        "99,99,99\n"
    )
    assert len(runs) == 1
    run = runs[0]
    assert (run.name, run.columns, run.ended) == ("t05", ["t_ms", "sensor", "x"], True)
    assert run.rows == [[12.0, 0.0, -3.0], [14.0, 1.0, 5.5]]
    assert run.comments == ["a comment"]
    assert run.column("x").tolist() == [-3.0, 5.5]


def test_parser_event_kinds_and_crlf():
    p = capture.StreamParser()
    assert p.feed("1,2,3\r\n").kind == "noise"
    assert p.feed("# end").kind == "noise"
    assert p.feed("# t09_piezo_dc,knock,sample,t_us,ch0,ch1,ch2,ch3\r\n").kind == "header"
    assert p.feed("# meta knock=1,rate_hz=100000,decim=1,pre=100,skew_ns=2500").kind == "comment"
    assert p.feed("1,0,-1000,2048,2047,2049,2048\r\n").kind == "row"
    assert p.feed("").kind == "noise"
    # A repeated identical header continues the same run.
    assert p.feed("# t09_piezo_dc,knock,sample,t_us,ch0,ch1,ch2,ch3").kind == "reheader"
    assert p.feed("2,0,-1000,2048,2047,2049,2048").kind == "row"
    assert p.feed("# knock 2 onsets_us,1.5,nan,3.0,4.5").kind == "comment"
    assert p.feed("# end\r\n").kind == "end"
    assert len(p.runs) == 1 and len(p.runs[0].rows) == 2
    assert p.runs[0].meta()["skew_ns"] == 2500
    onsets = p.runs[0].knock_notes("onsets_us")[2]
    assert onsets[0] == 1.5 and math.isnan(onsets[1])


def test_parser_skips_malformed_rows():
    run = capture.parse_text("# t,a,b\n1,2\n1,2,3\n48\nx,y\n3,4\n# end\n")[0]
    assert run.rows == [[1.0, 2.0], [3.0, 4.0]]
    assert run.bad_lines == 3


def test_parser_new_header_starts_a_new_run():
    runs = capture.parse_text("# a,x,y\n1,2\n# b,p\n5\n")
    assert [(r.name, r.ended, len(r.rows)) for r in runs] == [("a", False, 1), ("b", False, 1)]


def test_comment_lines_are_not_headers():
    p = capture.StreamParser()
    p.feed("# t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg")
    assert p.feed("# knock 1 peaks,1,2,3,4,5,6,7,8,9").kind == "comment"
    assert p.feed("# meta a=1,b=2").kind == "comment"
    assert len(p.runs) == 1


def test_committed_samples_match_the_generators():
    for name, make in make_data.SAMPLES.items():
        assert (DATA / name).read_text(encoding="utf-8") == make(), name


# ---------------------------------------------------------------- t06


def test_t06_summary_stream():
    run = capture.parse_file(DATA / "t06_noise.csv")[0]
    assert run.columns == ["sensor", "axis", "mean_mg", "rms_mg"]
    assert run.rows[0] == [0.0, "x", -12.41, 1.2]
    assert run.ended and run.bad_lines == 0
    summary = capture.noise_summary(run)
    assert sorted(summary) == [0, 1, 2, 3, 4, 5, 6, 8, 9]
    assert sorted(summary[0]) == ["mag", "x", "y", "z"]
    assert summary[4]["mag"][1] == pytest.approx(make_data.T06_RMS_MG[4])

    analysis = capture.analyze(run, capture.Options())
    d = analysis.data
    # Sensor 7 has no rows; sensor 3 is ten times noisier; 9 is the table.
    assert d["missing"] == [7]
    assert sorted(d["dynamic_rms"]) == [0, 1, 2, 3, 4, 5, 6, 8]
    assert d["noisy"] == [3] and d["noisiest"] == 3
    # The median is over the BMI160s only (the table reference would lower it).
    bmi = [make_data.T06_RMS_MG[s] * (10 if s == 3 else 1) for s in (0, 1, 2, 3, 4, 5, 6, 8)]
    assert d["median"] == pytest.approx(float(np.median(bmi)))
    # The threshold follows the typical sensor, not the outlier I3.
    assert d["threshold"] == pytest.approx(5 * float(np.median(bmi)))
    assert d["threshold_noisiest"] == pytest.approx(5 * 10 * make_data.T06_RMS_MG[3])
    assert d["table_reference"] == pytest.approx(make_data.T06_TABLE_RMS_MG)
    text = analysis.text
    for needle in (
        "MISSING BMI160 sensors (no rows = no samples): I7",
        "WARNING",
        "I3 (23.00 mg)",
        "table reference (LSM6DSL, not part of the median): dynamic RMS 0.90 mg",
        "5sigma",
        "over 8 sensor(s); noisiest I3 at 23.00 mg",
        "suggested trigger threshold: 12.2 mg (5 x the median dynamic RMS)",
        "for comparison, 5 x the noisiest BMI160 (I3) would be 115.0 mg",
    ):
        assert needle in text, needle
    # The table reference has its own row in the table.
    assert any(line.split()[:1] == ["table"] for line in text.splitlines())


def test_t06_clean_run_and_no_table_reference():
    text = "# t06_noise,sensor,axis,mean_mg,rms_mg\n"
    for s in range(9):
        text += f"{s},x,0.00,1.00\n{s},y,0.00,1.00\n{s},z,1000.00,1.00\n{s},mag,1000.00,{2 + s:.2f}\n"
    analysis = capture.analyze(capture.parse_text(text)[0], capture.Options())  # no '# end'
    assert analysis.data["missing"] == [] and analysis.data["noisy"] == []
    assert analysis.data["median"] == pytest.approx(6.0)
    assert analysis.data["threshold"] == pytest.approx(30.0)  # 5 x the median
    assert analysis.data["threshold_noisiest"] == pytest.approx(50.0)  # I8 at 10 mg
    assert "no BMI160 above" in analysis.text
    assert "table reference (sensor 9): no rows" in analysis.text
    assert "MISSING" not in analysis.text


def test_t06_rejects_other_layouts_and_bad_cells():
    old = capture.parse_text("# t06_noise,sensor,sample,x_mg,y_mg,z_mg\n0,0,1,2,3\n# end\n")[0]
    with pytest.raises(ValueError):
        capture.noise_summary(old)
    # A number where the axis word belongs, or a word where a number belongs.
    run = capture.parse_text(
        "# t06_noise,sensor,axis,mean_mg,rms_mg\n0,1,2,3\n0,x,abc,3\n0,mag,1,2\n"
    )[0]
    assert run.bad_lines == 2 and len(run.rows) == 1
    weird = capture.parse_text("# t06_noise,sensor,axis,mean_mg,rms_mg\n0,w,1,2\n")[0]
    with pytest.raises(ValueError):
        capture.noise_summary(weird)
    with pytest.raises(ValueError):
        weird.array()


# ---------------------------------------------------------------- t07


def test_t07_sample_matches_the_firmware_layout():
    lines = (DATA / "t07_capture.csv").read_text().splitlines()
    header = "# t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg"
    # The header at start-up, then again before each of the three knocks.
    assert lines[0] == lines[1] == header
    assert sum(line == header for line in lines) == 4
    for k in (2, 3):
        assert lines[lines.index(f"{k},0,-4,12,-7,1000") - 1] == header
    assert lines[-1] == "# end"
    runs = capture.parse_file(DATA / "t07_capture.csv")
    assert len(runs) == 1  # the repeats do not start new runs
    run = runs[0]
    assert run.ended and run.bad_lines == 0
    data = run.array()
    assert np.array_equal(data, np.rint(data))  # integers only
    assert sorted(set(data[:, 0].astype(int).tolist())) == [1, 2, 3]  # numbered from 1
    assert sorted(set(data[:, 2].astype(int).tolist())) == list(range(-4, 16))
    assert len(data) == 20 * (9 + 8 + 9)  # sensor 5 has no rows in knock 2
    notes = run.knock_notes("peaks")
    assert all(len(v) == 9 for v in notes.values())
    assert "# knock 2 peaks,2012,592,151,159,112,nan,339,947,349" in lines
    assert math.isnan(notes[2][5])  # absent = nan, not 0


def test_t07_old_format_file_still_loads():
    # First firmware version: the header once, 0 for an absent sensor.
    lines = (DATA / "t07_capture_old.csv").read_text().splitlines()
    assert sum(line.startswith("# t07_capture") for line in lines) == 1
    assert "# knock 2 peaks,2012,592,151,159,112,0,339,947,349" in lines
    assert not any("nan" in line for line in lines)
    old = capture.parse_file(DATA / "t07_capture_old.csv")
    new = capture.parse_file(DATA / "t07_capture.csv")
    assert len(old) == 1 and old[0].ended
    assert old[0].rows == new[0].rows
    a, b = capture.knock_peaks(old[0]), capture.knock_peaks(new[0])
    assert a.absent == b.absent == {2: [5]} and a.incomplete == b.incomplete == []
    for k in (1, 2, 3):
        assert np.array_equal(a.peaks[k], b.peaks[k], equal_nan=True)
    opts = capture.Options(mode="gain", sensor=5)
    assert capture.analyze(old[0], opts).text == capture.analyze(new[0], opts).text


def test_t07_nan_peak_is_absent_even_with_rows():
    # `nan` is never a measurement; a 0 with rows still is (see below).
    text = "# t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg\n"
    for s in (0, 1):
        text += "".join(f"1,{s},{i},0,0,1000\n" for i in range(-4, 16))
    text += "# knock 1 peaks,0,nan,NaN,nan,nan,nan,nan,nan,nan\n# end\n"
    found = capture.knock_peaks(capture.parse_text(text)[0])
    assert found.peaks[1][0] == 0.0
    assert found.absent == {1: [1, 2, 3, 4, 5, 6, 7, 8]}
    assert "peaks [mg] 0 - - " in capture.knock_summary(capture.parse_text(text)[0], 1, capture.Options())


def test_t07_recording_that_starts_in_mid_run(tmp_path):
    # The script was attached while knock 1 was being sent: the start-up
    # header and knock 1's header are missed, knocks 2 and 3 are still saved.
    lines = (DATA / "t07_capture.csv").read_text(encoding="utf-8").splitlines()
    header = lines[0]
    second = [i for i, line in enumerate(lines) if line == header][2]  # knock 2's header
    late = ["20,-7,1000"] + lines[40:]  # starts in the middle of a row of knock 1
    out = io.StringIO()
    session = capture.LiveSession(tmp_path, capture.Options(), plot=False, out=out, timestamp=lambda: "TS")
    for line in late:
        session.feed_line(line + "\r\n")
    saved = tmp_path / "t07_capture-TS.csv"
    assert session.saved == [saved]  # one run, one file
    assert saved.read_text(encoding="utf-8").splitlines() == lines[second:]
    run = capture.parse_file(saved)[0]
    assert run.ended and run.bad_lines == 0
    found = capture.knock_peaks(run)
    assert list(found.peaks) == [2, 3] and found.incomplete == []
    assert found.absent == {2: [5]}
    printed = out.getvalue()
    assert "t07_capture knock 2: peaks [mg]" in printed and "absent: I5" in printed
    assert "t07_capture knock 3: peaks [mg]" in printed
    assert "knock 1" not in printed
    assert "2 knock(s)" in printed

    # The old firmware sent the header once: the same late start saves nothing.
    old = (DATA / "t07_capture_old.csv").read_text(encoding="utf-8").splitlines()
    session = capture.LiveSession(tmp_path / "old", capture.Options(), plot=False, out=io.StringIO())
    for line in old[40:]:
        session.feed_line(line)
    assert session.saved == []


def test_t07_live_session_keeps_repeated_headers_in_one_file(tmp_path):
    text = (DATA / "t07_capture.csv").read_text(encoding="utf-8")
    out = io.StringIO()
    session = capture.LiveSession(tmp_path, capture.Options(), plot=False, out=out, timestamp=lambda: "TS")
    for line in text.splitlines():
        session.feed_line(line + "\r\n")
    assert session.saved == [tmp_path / "t07_capture-TS.csv"]
    assert session.saved[0].read_text(encoding="utf-8") == text  # verbatim, headers included
    assert out.getvalue().count("recording t07_capture") == 1
    assert "3 knock(s)" in out.getvalue()


def test_t07_absent_sensor_is_not_a_measurement():
    run = capture.parse_file(DATA / "t07_capture.csv")[0]
    found = capture.knock_peaks(run)
    assert list(found.peaks) == [1, 2, 3]
    assert found.absent == {2: [5]} and found.incomplete == []
    assert math.isnan(found.peaks[2][5]) and found.peaks[1][5] > 0
    assert found.always_absent() == []
    analysis = capture.analyze(run, capture.Options(mode="gain", sensor=5))
    g = analysis.data["gain"]
    # Two knocks count for I5, and its mean is not dragged down by a 0.
    assert (g["knocks"], g["total_knocks"]) == (2, 3)
    assert g["mean"] == pytest.approx((found.peaks[1][5] + found.peaks[3][5]) / 2)
    assert "absent in knock 2: I5" in analysis.text
    assert "I5 was absent in 1 knock(s)" in analysis.text


def test_t07_truncated_file_without_end():
    run = capture.parse_file(DATA / "t07_capture_truncated.csv")[0]
    assert not run.ended
    found = capture.knock_peaks(run)
    # Knock 3 was cut off before its peaks line; sensor 6 never had rows.
    assert list(found.peaks) == [1, 2] and found.incomplete == [3]
    assert found.always_absent() == [6]
    analysis = capture.analyze(run, capture.Options(mode="gain", sensor=2))
    g = analysis.data["gain"]
    assert g["knocks"] == 2 and g["loudest"] == 2 and g["absent"] == [6]
    assert math.isnan(g["means"][6])
    for needle in (
        "absent in every knock (no rows, no peak): I6",
        "incomplete knock(s) left out",
        "max at",
    ):
        assert needle in analysis.text, needle
    assert "I6=" not in analysis.text
    with pytest.raises(ValueError):
        capture.analyze(run, capture.Options(mode="gain", sensor=6))
    assert capture.main(["--replay", str(DATA / "t07_capture_truncated.csv"), "--no-plot"]) == 0


def test_t07_zero_peak_with_rows_is_kept():
    # A real 0 (the sensor has rows) stays a measurement.
    text = "# t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg\n"
    text += "".join(f"1,0,{i},0,0,1000\n" for i in range(-4, 16))
    text += "# knock 1 peaks,0,0,0,0,0,0,0,0,0\n"
    found = capture.knock_peaks(capture.parse_text(text)[0])
    assert found.peaks[1][0] == 0.0 and found.absent == {1: [1, 2, 3, 4, 5, 6, 7, 8]}


def test_t07_peaks_from_firmware_lines_and_from_rows_agree():
    with_lines = capture.parse_text(make_data.t07_capture(near=3, knocks=2))[0]
    without = capture.parse_text(make_data.t07_capture(near=3, knocks=2, with_peaks=False))[0]
    a = capture.knock_peaks(with_lines).peaks
    b = capture.knock_peaks(without).peaks
    assert list(a) == list(b) == [1, 2]
    for k in a:
        assert len(a[k]) == 9
        assert b[k] == pytest.approx(a[k], abs=1.0)
        assert int(np.argmax(a[k])) == 3


def test_t07_gain_mode():
    run = capture.parse_text(make_data.t07_capture(near=4, knocks=20))[0]
    analysis = capture.analyze(run, capture.Options(mode="gain", sensor=4))
    g = analysis.data["gain"]
    assert g["knocks"] == 20 and g["loudest"] == 4
    assert g["mean"] == pytest.approx(2000.0 / make_data.TRUE_GAINS[4], rel=0.03)
    assert g["cv"] < 0.05
    assert "20 knocks near sensor I4" in analysis.text
    assert "WARNING" not in analysis.text and "absent" not in analysis.text
    wrong = capture.analyze(run, capture.Options(mode="gain", sensor=0))
    assert "WARNING" in wrong.text
    with pytest.raises(ValueError):
        capture.analyze(run, capture.Options(mode="gain"))


def test_solve_gains_normalisation():
    gains = capture.solve_gains([100.0, 200.0, 400.0])
    assert gains.mean() == pytest.approx(1.0)
    assert gains * np.array([100.0, 200.0, 400.0]) == pytest.approx([gains[0] * 100.0] * 3)
    with pytest.raises(ValueError):
        capture.solve_gains([1.0, 0.0])


def test_gain_solve_recovers_the_true_gains():
    runs = [capture.parse_text(make_data.t07_capture(near=k, knocks=20))[0] for k in range(9)]
    center = capture.parse_text(make_data.t07_capture(near=None, knocks=5))[0]
    analysis = capture.analyze_gain_solve(runs, capture.Options(), center)
    expected = make_data.TRUE_GAINS / make_data.TRUE_GAINS.mean()
    assert analysis.data["gains"] == pytest.approx(expected, rel=0.02)
    assert analysis.data["gains"].mean() == pytest.approx(1.0)
    # After calibration a centre knock is symmetric.
    sym = analysis.data["center_symmetry"]
    assert sym["I1/I5"] == pytest.approx(1.0, abs=0.03)
    assert sym["I3/I7"] == pytest.approx(1.0, abs=0.03)
    assert sym["corner_spread"] < 0.05
    assert analysis.data["center_pass"] is True
    for needle in ("I1/I5", "I3/I7", "corners I0/I2/I4/I6", "spread", "T7"):
        assert needle in analysis.text
    with pytest.raises(ValueError):
        capture.analyze_gain_solve(runs[:8], capture.Options())


def test_gain_solve_ignores_absent_sensors():
    # In the "near sensor 2" file sensor 7 is absent throughout, and sensor 2
    # itself misses knock 1: neither may enter a mean as 0.
    texts = [make_data.t07_capture(near=k, knocks=20) for k in range(9)]
    texts[2] = make_data.t07_capture(near=2, knocks=20, absent={0: (7,), 1: (2,)})
    runs = [capture.parse_text(t)[0] for t in texts]
    analysis = capture.analyze_gain_solve(runs, capture.Options())
    expected = make_data.TRUE_GAINS / make_data.TRUE_GAINS.mean()
    assert analysis.data["gains"] == pytest.approx(expected, rel=0.02)
    assert "loudest is" not in analysis.text
    # The target sensor absent in every knock of its own file is an error.
    texts[4] = make_data.t07_capture(near=4, knocks=3, absent={0: (4,)})
    runs = [capture.parse_text(t)[0] for t in texts]
    with pytest.raises(ValueError, match="I4"):
        capture.analyze_gain_solve(runs, capture.Options())


# ---------------------------------------------------------------- piezo


def test_t09_summary_and_rail_flag():
    run = capture.parse_file(DATA / "t09_piezo_dc.csv")[0]
    analysis = capture.analyze(run, capture.Options())
    knocks = analysis.data["knocks"]
    assert [k.knock for k in knocks] == [1, 2]
    assert knocks[0].pre == 30 and knocks[0].counts.shape == (200, 4)
    assert knocks[0].baseline() == pytest.approx([2051, 2045, 2051, 2045], abs=1.0)
    assert 600 < knocks[0].peak_dev()[0] < 950
    # Knock 2 is four times as hard and clips at the ADC rails.
    assert knocks[1].counts.max() == 4095 and knocks[1].counts.min() == 0
    assert "*" in analysis.text
    assert "onset" not in analysis.text


def test_onset_position_matches_the_firmware_rule():
    # Baseline 2048 +- 1 (sigma 1): threshold 8; deviations 4 -> 16 cross it
    # a third of the way from sample 9 to 10 (same case as the Rust test).
    s = np.array([2047, 2049] * 4 + [2048, 2052, 2064, 2200], dtype=float)
    assert capture.onset_position(s, 8, 8.0, 2.0) == pytest.approx(9 + 4 / 12)
    flat = np.array([2048, 2048, 2048, 2048, 2049, 2048, 2047, 2060, 2100], dtype=float)
    assert capture.onset_position(flat, 4, 8.0, 5.0) == pytest.approx(6 + 4 / 11)
    assert math.isnan(capture.onset_position(np.full(16, 2048.0), 4, 8.0, 5.0))
    assert math.isnan(capture.onset_position(flat, 0, 8.0, 5.0))


def test_t10_onsets_order_and_v_eff():
    run = capture.parse_file(DATA / "t10_piezo_time.csv")[0]
    analysis = capture.analyze(run, capture.Options(known_pos=make_data.T10_POS_CM))
    onsets = analysis.data["onsets_us"]
    truth = make_data.piezo_arrivals_us()
    # Knocks 1 and 2 carry firmware onsets, knock 3 is computed on the host.
    assert onsets[1] - onsets[1].min() == pytest.approx(truth, abs=0.06)
    host = onsets[3]
    assert np.argsort(host).tolist() == np.argsort(truth).tolist()
    # Threshold crossing lags the true arrival by a similar amount on every
    # channel, so the differences survive to within a couple of samples.
    assert host - host.min() == pytest.approx(truth, abs=25.0)
    assert analysis.data["v_eff_m_s"] == pytest.approx(make_data.T10_V_M_S, rel=0.10)
    assert "median v_eff" in analysis.text and " fw " in analysis.text and " host " in analysis.text
    exact = capture.estimate_v_eff(truth, make_data.T10_POS_CM)
    assert exact["v_m_s"] == pytest.approx(make_data.T10_V_M_S, rel=1e-6)


def test_host_onsets_apply_the_scan_skew():
    # The same step on every channel in the same row: channel k was really
    # sampled k * 2.5 us later, so its onset is reported later by that much.
    t = (np.arange(60) - 20) * 10.0
    counts = np.full((60, 4), 2048.0)
    counts[30:, :] = 2548.0
    onsets = capture.host_onsets_us(capture.PiezoKnock(1, t, counts))
    assert np.diff(onsets) == pytest.approx([2.5, 2.5, 2.5])
    assert 90.0 < onsets[0] <= 100.0


def test_v_eff_degenerate_cases():
    centre = capture.estimate_v_eff(np.array([10.0, 11.0, 10.5, 9.5]), (0.0, 0.0))
    assert math.isnan(centre["v_m_s"]) and "equidistant" in centre["reason"]
    few = capture.estimate_v_eff(np.array([10.0, np.nan, np.nan, 9.5]), (2.0, 1.0))
    assert math.isnan(few["v_m_s"])
    backwards = capture.estimate_v_eff(-make_data.piezo_arrivals_us(), make_data.T10_POS_CM)
    assert math.isnan(backwards["v_m_s"]) and "contradicts" in backwards["reason"]


def test_t10_centre_knock_reports_the_spread():
    run = capture.parse_text(make_data.piezo_stream("t10_piezo_time", knocks=1, pos_cm=(0.0, 0.0)))[0]
    analysis = capture.analyze(run, capture.Options(known_pos=(0.0, 0.0)))
    assert "v_eff_m_s" not in analysis.data
    assert "arrival spread" in analysis.text


def test_t10_long_window_ringdown_table():
    run = capture.parse_file(DATA / "t10_piezo_time_long.csv")[0]
    analysis = capture.analyze(run, capture.Options())
    assert "decimated by 25" in analysis.text
    assert "ring-down [ms]" in analysis.text and "refractory" in analysis.text
    assert "onsets_us" not in analysis.data
    ring = analysis.data["ringdown_us"][1]
    assert ring[0] < ring[3] < ring[1] < ring[2]
    assert 15_000 < ring[0] < 25_000


# ---------------------------------------------------------------- output


def test_knock_summary_lines():
    t10 = capture.parse_file(DATA / "t10_piezo_time.csv")[0]
    line = capture.knock_summary(t10, 1, capture.Options())
    assert line.startswith("t10_piezo_time knock 1: 200 rows") and "first P1" in line
    t07 = capture.parse_file(DATA / "t07_capture.csv")[0]
    line = capture.knock_summary(t07, 2, capture.Options())
    assert "max at I0" in line and "absent: I5" in line
    assert "absent" not in capture.knock_summary(t07, 1, capture.Options())


def test_live_session_saves_verbatim_and_summarises(tmp_path):
    text = (DATA / "t10_piezo_time.csv").read_text(encoding="utf-8")
    out = io.StringIO()
    session = capture.LiveSession(tmp_path, capture.Options(), plot=False, out=out, timestamp=lambda: "TS")
    session.feed_line("garbage before the header\r\n")
    for line in text.splitlines():
        session.feed_line(line + "\r\n")
    saved = tmp_path / "t10_piezo_time-TS.csv"
    assert session.saved == [saved]
    assert saved.read_text(encoding="utf-8") == text
    printed = out.getvalue()
    for k in (1, 2, 3):
        assert f"t10_piezo_time knock {k}: 200 rows" in printed
    assert "onset times" in printed


def test_live_session_idle_and_interrupt(tmp_path):
    lines = (DATA / "t09_piezo_dc.csv").read_text(encoding="utf-8").splitlines()
    out = io.StringIO()
    session = capture.LiveSession(tmp_path, capture.Options(), plot=False, out=out, timestamp=lambda: "TS")
    for line in lines[:150]:
        session.feed_line(line)
    assert "knock 1" not in out.getvalue()
    session.idle()  # the line went quiet: knock 1 is summarised
    assert "t09_piezo_dc knock 1: 148 rows" in out.getvalue()
    session.finish()  # Ctrl-C in the middle of the run
    assert "no '# end' line" in out.getvalue()
    assert len((tmp_path / "t09_piezo_dc-TS.csv").read_text().splitlines()) == 150


def test_plots_are_written_beside_the_csv(tmp_path):
    for name in make_data.SAMPLES:
        target = tmp_path / name
        target.write_bytes((DATA / name).read_bytes())
        assert capture.main(["--replay", str(target)]) == 0
    pngs = sorted(p.name for p in tmp_path.glob("*.png"))
    assert "t06_noise.png" in pngs and "t07_capture.png" in pngs
    assert "t09_piezo_dc-knock1.png" in pngs and "t10_piezo_time-knock3.png" in pngs
    assert all((tmp_path / p).stat().st_size > 5000 for p in pngs)


def test_cli_replay_and_errors(tmp_path, capsys):
    assert capture.main(["--replay", str(DATA / "t10_piezo_time.csv"), "--no-plot", "--known-pos", "2,1"]) == 0
    assert "median v_eff" in capsys.readouterr().out
    assert not list(DATA.glob("*.png"))
    empty = tmp_path / "empty.csv"
    empty.write_text("nothing here\n")
    assert capture.main(["--replay", str(empty), "--no-plot"]) == 2
    assert capture.main([]) == 2
    assert capture.main(["--mode", "gain-solve"]) == 2


def test_cli_gain_solve(tmp_path, capsys):
    files = []
    for k in range(9):
        path = tmp_path / f"near{k}.csv"
        path.write_text(make_data.t07_capture(near=k, knocks=20), encoding="utf-8")
        files.append(str(path))
    assert capture.main(["--mode", "gain-solve", "--no-plot", *files]) == 0
    out = capsys.readouterr().out
    assert "c_i" in out and "I1/I5" in out
