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


def test_t06_rms_table():
    run = capture.parse_file(DATA / "t06_noise.csv")[0]
    rms = capture.noise_rms(run)
    assert sorted(rms) == list(range(9))
    for s, values in rms.items():
        assert values == pytest.approx([make_data.T06_RMS_MG[s]] * 3, abs=2e-3)
    text = capture.analyze(run, capture.Options()).text
    assert "I8" in text and "rms_z" in text and "[mg]" in text


def test_t06_accepts_raw_axis_names_and_rejects_others():
    run = capture.parse_text("# t06_noise,t_ms,sensor,x,y,z\n0,0,1,2,3\n1,0,3,2,1\n# end\n")[0]
    assert capture.noise_rms(run)[0] == pytest.approx([1.0, 0.0, 1.0])
    bad = capture.parse_text("# t06_noise,a,b\n1,2\n# end\n")[0]
    with pytest.raises(ValueError):
        capture.noise_rms(bad)


# ---------------------------------------------------------------- t07


def test_t07_peaks_from_firmware_lines_and_from_rows_agree():
    with_lines = capture.parse_text(make_data.t07_capture(near=3, knocks=2))[0]
    without = capture.parse_text(make_data.t07_capture(near=3, knocks=2, with_peaks=False))[0]
    a = capture.knock_peaks(with_lines)
    b = capture.knock_peaks(without)
    assert list(a) == list(b) == [1, 2]
    for k in a:
        assert len(a[k]) == 9
        assert b[k] == pytest.approx(a[k], abs=0.1)
        assert int(np.argmax(a[k])) == 3


def test_t07_gain_mode():
    run = capture.parse_text(make_data.t07_capture(near=4, knocks=20))[0]
    analysis = capture.analyze(run, capture.Options(mode="gain", sensor=4))
    g = analysis.data["gain"]
    assert g["knocks"] == 20 and g["loudest"] == 4
    assert g["mean"] == pytest.approx(2000.0 / make_data.TRUE_GAINS[4], rel=0.03)
    assert g["cv"] < 0.05
    assert "20 knocks near sensor I4" in analysis.text
    assert "WARNING" not in analysis.text
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
    assert "max at I0" in capture.knock_summary(t07, 2, capture.Options())


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
