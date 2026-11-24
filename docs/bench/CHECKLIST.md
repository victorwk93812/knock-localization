# Bench test checklist

Procedures: [README.md](README.md) (setup), [materials.md](materials.md) (meter checks,
soldering) and one page per test in [tests/](tests/). Tick a box only when the pass criterion is
met; write the measured value where there's a blank.

Date started: ________  Tested by: ________  Computer (OS): ________

---

## M1 — Breadboard bring-up

### Tools and setup (README §2, materials §1)
- [ ] Multimeter continuity beeps, < 0.5 Ω
- [ ] USB cable enumerates the ST-LINK (`0483:374b`)
- [ ] Board 3V3 = ______ V (3.25–3.35) · 5V = ______ V
- [ ] `probe-rs list` shows the probe; serial port found: ______
- [ ] `cargo build --release --bins` succeeds in `firmware/`; `uv run pytest` passes in `host/`

### Passive parts (materials §2)
- [ ] 6 × 1 MΩ, 6 × 10 kΩ, 4 × 100 kΩ within ±1 %
- [ ] 4 × 100 nF, 6 × 1 nF (codes 104 / 102 checked)
- [ ] 2 × 10 µF, polarity stripe identified
- [ ] 10 × 1N4148: forward 0.55–0.70 V, reverse open. Typical: ______ V
- [ ] 6 × piezo discs with leads soldered (materials §5): open circuit, capacitance
      in range, tap responds, leads hold

  | Disc | 1 | 2 | 3 | 4 | 5 | 6 |
  |---|---|---|---|---|---|---|
  | nF | | | | | | |

### Wires and breadboard (materials §3)
- [ ] All female–female 30 cm wires pass continuity + wiggle. Binned: ______
- [ ] All short jumpers pass. Binned: ______
- [ ] Breadboard rows and rails checked; rails bridged if split

### BMI160 modules: preparation and T1 (materials §4, tests/t01_chip_id.md)

| Module | Marking OK | VIN–GND not short | Pull-up Ω | CS→3V3 | SAO strap | 3V3 pin (V) | Address | CHIP_ID | \|a\| (g) | mA |
|---|---|---|---|---|---|---|---|---|---|---|
| 0 | | | | | GND | | 0x68 | | | |
| 1 | | | | | GND | | 0x68 | | | |
| 2 | | | | | GND | | 0x68 | | | |
| 3 | | | | | GND | | 0x68 | | | |
| 4 | | | | | GND | | 0x68 | | | |
| 5 | | | | | GND | | 0x68 | | | |
| 6 | | | | | GND | | 0x68 | | | |
| 7 | | | | | GND | | 0x68 | | | |
| 8 | | | | | 3V3 | | 0x69 | | | |
| 9 (spare) | | | | | GND | | 0x68 | | | |

- [ ] Headers soldered and trimmed flush (or wires soldered directly); straps
      checked with the meter
- [ ] All modules labeled
- [ ] **T1 pass:** every module returns `0xD1` at the expected address

### Firmware tests
- [ ] **t00 board self-check:** LSM6DSL `0x6A`, |a| = ______ g, serial echo clean
      at 921 600 baud (if not, highest clean rate: ______)
- [ ] **T2 (t02):** mux at 0x70, A0–A2 low, RESET high; channels 0–7 each show
      0x68; 0x69 and 0x70 always visible. Failing channels: ______
- [ ] **T3 (t03):** 10⁶ reads at 400 kHz, errors: ______ (must be 0).
      Fallback used (100 kHz / extra pull-ups): ______
- [ ] **T4 (t04):** 10 min, FIFO overflows: ______ (must be 0) ·
      loop time max: ______ ms (< 20) · max FIFO level: ______ frames
- [ ] **T14:** total 3.3 V current with everything running: ______ mA
- [ ] **T5 (t05), rate:** every sensor within 400 ± 2 %. Range: ______ – ______ /s
- [ ] **T6 (t06), breadboard:** noise RMS typical: ______ mg ·
      noisiest sensor: I__ at ______ mg

### Piezo front end: T9 (tests/t09_piezo_dc.md)
- [ ] STM32L475 pin voltage limit checked against the 3.95 V clamp level.
      Datasheet limit: ______ V · built with: ______ kΩ / ______ pF
- [ ] VMID = ______ V (1.65 ± 0.05)
- [ ] Clamp diode direction verified on all 4 channels (8 diodes)
- [ ] ADCj nodes on the meter (≈ 1.5 V expected): ______ / ______ / ______ / ______
- [ ] t00 with the front end connected: A0–A3 ≈ 2048 counts
- [ ] **T9 (t09):** sample rate ______ S/s (100 000 ± 1 %) ·
      idle counts ______ / ______ / ______ / ______ (2048 ± 150) ·
      idle RMS ______ / ______ / ______ / ______ (< 8) ·
      each channel answers its own disc ·
      hardest knock: peak ______ counts, flat-top / rail: ______, board keeps running

**M1 done:** ________ (date)

---

## M2 — Plate assembly

- [ ] **T8 (t07, on an offcut):** ratio I0 / I8 with both on tape: ______ ·
      with I0 on cyanoacrylate: ______ · chosen mounting: ______
- [ ] Paper template printed 1:1 and taped under the plate
- [ ] 9 modules glued at the cell centers, same orientation
- [ ] 4 piezos glued at (±5, ±3.33) cm, brass face down
- [ ] Cables taped flat, one exit edge, strain relief at the plate edge and breadboard
- [ ] 4 foam feet, about 2 cm in from each corner
- [ ] t02 re-run on the assembled plate: all sensors still found
- [ ] **T5 (t05), knocks, mounted:** hardest knock peak: ______ g on I__ (< 15.5)
- [ ] **T6 (t06), mounted:** median noise RMS ______ mg · suggested threshold
      (5 × median): ______ mg · noisiest sensor would need: ______ mg (I__)
- [ ] **T7 (t07):** 20 knocks per sensor recorded, 9 files + center file.
      Largest CV: ______ % · files: ______

  | Sensor | I0 | I1 | I2 | I3 | I4 | I5 | I6 | I7 | I8 |
  |---|---|---|---|---|---|---|---|---|---|
  | Gain c_i | | | | | | | | | |

- [ ] Symmetry after calibration: I1 vs I5 ______ % · I3 vs I7 ______ % ·
      corners spread ______ % (all < 10 %)
- [ ] **T10 (t10):** nearest piezo first at P0 / P1 / P2 / P3 ·
      center knock arrival spread: ______ µs (< 100) · v_eff ≈ ______ m/s
- [ ] **T11 (t10 long window):** ring-down to below threshold: ______ ms →
      refractory period: ______ ms

**M2 done:** ________ (date)

---

## M6 — Rehearsal

- [ ] **T13 (t13):** 30 min run; SDA pulled and restored: recovered in ______ s ·
      power wire pulled and restored: recovered in ______ s · bus recoveries: ______ · no watchdog reset
- [ ] **T12 latency** (needs the acquisition firmware and avatar): ______ ms (≤ 100)
- [ ] **T12 false triggers per 10 min:** idle ______ · typing on the table ______ ·
      table bumps rejected ______ / ______

**M6 done:** ________ (date)
