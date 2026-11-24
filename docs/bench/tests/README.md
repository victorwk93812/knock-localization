# Test pages

One page per test binary. Setup, the log format and the capture script are
explained once in [../README.md](../README.md) §3.

| Binary | Plan test | Proves | Needs wired |
|---|---|---|---|
| [t00_board](t00_board.md) | pre-check | Board, probe, serial port and tools work | nothing |
| [t01_chip_id](t01_chip_id.md) | T1 | Each BMI160 module is genuine, strapped right and reads 1 g | one module |
| [t02_mux_scan](t02_mux_scan.md) | T2 | Each multiplexer channel reaches one sensor; I8 is on the trunk | multiplexer + 9 modules |
| [t03_bus_soak](t03_bus_soak.md) | T3 | The bus makes 10⁶ transfers without an error | same |
| [t04_throughput](t04_throughput.md) | T4, T14 | The firmware keeps up with nine sensors at 400 Hz; supply current | same |
| [t05_odr_sat](t05_odr_sat.md) | T5 | Sample rate is right; the hardest knock doesn't saturate | same, mounted for the knock part |
| [t06_noise](t06_noise.md) | T6 | Noise of every sensor; trigger threshold | same |
| [t07_capture](t07_capture.md) | T7, T8 | Knock waveforms: mounting comparison, gain calibration | 2 modules (T8) or all (T7) |
| [t09_piezo_dc](t09_piezo_dc.md) | T9 | Piezo front end idles at mid-scale and survives a hard knock | piezo front end |
| [t10_piezo_time](t10_piezo_time.md) | T10, T11 | Arrival order and time differences; ring-down | piezos on the plate |
| [t13_recovery](t13_recovery.md) | T13 | The firmware survives unplugged sensors for 30 min | everything |

There is no t08, t11, t12 or t14 binary: T8 uses t07, T11 uses t10, T14 is a
meter reading during t04, and T12 waits for the real firmware.
