# Bench test guide

How to check every part and run every test in `Final_Proposal_v2_EN.md` §8.
Tick things off in [CHECKLIST.md](CHECKLIST.md) as you go.

Work in the order of the checklist and **add one thing at a time**. When a test
fails, the fault is in the thing you just added.

> **Status of the firmware tests.** Nothing here has run on the board yet; the test
> binaries are only known to compile. Expect to find firmware bugs as well as
> hardware faults. When a test fails in a way this guide doesn't explain, paste the
> terminal output to Claude.

---

## 1. One-time setup

| Step | Command | Expect |
|---|---|---|
| Toolchain | `rustup toolchain install` (in the repo) | 1.99.1 with `thumbv7em-none-eabihf` |
| Probe | plug in the Discovery board, `probe-rs list` | one `STLink V2-1` probe |
| Serial port | `ls /dev/ttyACM*` | `/dev/ttyACM0` |
| Python tools | `cd host && uv sync` | venv created |

On this machine the probe-rs udev rules are installed and your user is in the `uucp`
group, so no `sudo` is needed.

**Running a test**

```sh
cd firmware
cargo run --release --bin t02_mux_scan
```

This builds, flashes and then prints the test's log in the same terminal. Every test
prints a banner, one `PASS`/`FAIL` line per check, and ends with
`RESULT: PASS` or `RESULT: FAIL (n failed)`. LED2 then blinks slowly for a pass and
fast for a fail. Stop with Ctrl-C. Press the black reset button to run it again.

**Tests that record waveforms** also send CSV over the serial port. Start the
capture script in a second terminal *before* knocking:

```sh
cd host
uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench/
```

It saves one file per run and shows a plot.

**Multimeter habits**

- Probe tips are enough. For a female Dupont socket, push a male header pin in and
  probe the pin.
- For current, the meter goes in series, red lead in the mA jack. **Move the lead
  back to V/Ω afterwards.** Probing a supply with the lead in the mA jack shorts it.

---

## 2. Material checks (no firmware)

Do these with the multimeter, power off, parts out of circuit.

### 2.1 Tools

| Item | Test | Pass |
|---|---|---|
| Multimeter | Continuity mode, touch probes together | Beep, < 0.5 Ω |
| USB cable | Plug in the board, `lsusb` | `0483:374b ST-LINK/V2.1`. Only an LED and no device = charge-only cable |
| Board power | DC volts, Arduino header 3V3–GND and 5V–GND | 3.25–3.35 V, ≈ 5 V |

### 2.2 Passive parts

| Part | How | Pass |
|---|---|---|
| Resistors 10 kΩ, 100 kΩ, 1 MΩ | Ω mode. Don't hold both leads of the 1 MΩ: your fingers read in parallel | Within ±1 % (metal film) |
| 100 nF (`104`), 1 nF (`102`) | Capacitance mode; zero the meter with open leads first | 100 nF within ±20 %. 1 nF is hard to read on cheap meters; the printed code is enough |
| 10 µF electrolytic | Capacitance mode. Stripe = **negative** lead | 8–12 µF |
| 1N4148 diodes | Diode mode. Red on the plain end, black on the banded end; then swap | **0.55–0.70 V** forward, `OL` reversed. ≈ 0.2–0.3 V would be a Schottky: wrong part |
| Piezo discs (after soldering leads) | Ω: open (> 10 MΩ). Capacitance: roughly 10–30 nF. DC volts: tap it, reading jumps. Tug each lead gently | All four true. ≈ 0 Ω or ≈ 0 nF = cracked or shorted disc |

### 2.3 Wires and breadboard

- **Every Dupont wire:** continuity end to end (< 1 Ω), then bend the wire next to
  each crimp while the meter beeps. A stutter means a bad crimp: bin it.
- **Female ends:** push onto a header pin. A loose socket causes random I2C errors
  later (T3, T13). Bin it.
- **Breadboard:** each 5-hole row conducts; neighboring rows don't. Check the power
  rails end to end; bridge them if they're split in the middle.

### 2.4 GY-BMI160 module preparation (all 10)

1. **Read the chip marking** with a phone macro photo. Note anything that isn't a
   BMI160.
2. **Short check:** Ω between VIN and GND, and 3V3 and GND. Must not be ≈ 0.
3. **Pull-ups:** Ω from SDA to 3V3 and from SCL to 3V3. Write the value down; it
   helps diagnose T3. Each mux channel sees only its own module's pull-ups, and
   the trunk sees those of module 8 plus the mux board's.
4. **Straps, on the module:**
   - **CS → 3V3.** Without it the chip can start in SPI mode and never answer.
   - **SAO → GND** on modules 0–7 (address 0x68). **SAO → 3V3** on module 8 (0x69).
   - Module 9 is the spare: strap it like 0–7.
5. **Header:** solder from the component side and trim the tails flush underneath
   so the PCB sits flat on the acrylic. Or solder four wires directly.
6. **Supply:** the module has a small regulator between VIN and 3V3. Power it from
   the board's 3.3 V, then measure the module's **3V3 pin**. If it reads below
   ≈ 3.0 V when fed through VIN, feed the 3V3 pin directly instead.
7. **Label** each module 0–9 with tape.

---

## 3. Firmware tests

Wiring names: D14 = SDA, D15 = SCL (Arduino header, I2C1). "Trunk" means those two
lines before the multiplexer.

### t00 — board self-check

- **Purpose:** prove the board, probe, serial port and onboard sensor work before
  anything is attached.
- **Wiring:** USB only.
- **Run:** `cargo run --release --bin t00_board`. In a second terminal open the
  serial port (`uv run bench/capture.py --port /dev/ttyACM0 --echo`) and type: the
  characters should come back.
- **Pass:** LED2 blinks; LSM6DSL answers `0x6A`; |a| between 0.8 and 1.2 g; serial
  echo is clean at 921 600 baud.
- **If it fails:** no probe → cable or udev; garbled echo → note it, we lower the
  baud rate.

### t01 — T1: incoming module check (one module at a time)

- **Purpose:** every BMI160 answers, has the right ID and address, and reads 1 g.
- **Wiring (no multiplexer):**

  ```
  module VIN (or 3V3) ── 3V3        module SCL ── D15
  module GND ─────────── GND        module SDA ── D14
  CS and SAO already strapped on the module (§2.4)
  ```
- **Run:** `cargo run --release --bin t01_chip_id`. Lay the module flat. After each
  result, swap in the next module and press the **blue user button**.
- **Also measure once per module:** supply current, meter in series in the 3V3
  wire. Expect about 1 mA or less (accelerometer only), plus any power LED.
- **Pass (per module):** address `0x68` (modules 0–7, 9) or `0x69` (module 8);
  `CHIP_ID = 0xD1`; |a| within 0.9–1.1 g with z ≈ ±1 g and x, y ≈ 0.
- **If it fails:**
  - nothing found → CS strap, power, SDA/SCL swapped, bad wire;
  - wrong address → SAO strap;
  - wrong ID → different chip, set the module aside;
  - |a| far from 1 g → damaged sensor.

### t02 — T2: multiplexer and I8

- **Purpose:** each mux channel reaches exactly one sensor, and I8 is visible
  beside the mux.
- **Wiring:** TCA9548A VCC/GND to the rails, its SDA/SCL to D14/D15. Module k to
  channel k (SDk/SCk) for k = 0…7. Module 8 (0x69) to the **trunk**, in parallel
  with the mux. Before powering, check with the meter that the mux's A0–A2 read low
  and RESET reads high (through the module's own resistors).
- **Run:** `cargo run --release --bin t02_mux_scan`. Tip: first run it with only
  the mux and one known-good module, moving that module through channels 0–7.
- **Pass:** mux at `0x70`; every channel shows `0x68` with `CHIP_ID 0xD1`; `0x69`
  and `0x70` visible on every channel; with all channels off, `0x68` is absent.
- **If it fails:** one dead channel → that wire or that module; `0x68` visible with
  all channels off → a 0x68 module is on the trunk by mistake; readback wrong →
  mux RESET floating or mux not powered.

### t03 — T3: bus integrity soak

- **Purpose:** the I2C bus is error-free with all nine sensors on 30 cm wires.
- **Wiring:** as t02, with the final 30 cm wires.
- **Run:** `cargo run --release --bin t03_bus_soak` and leave it about 30 minutes.
  Wiggle nothing. If you have the logic analyzer, look at SCL/SDA edges on the
  trunk and on one channel.
- **Pass:** 10⁶ reads, 0 errors on every sensor.
- **If it fails:** errors on one sensor → its wires; errors everywhere → pull-ups.
  Too weak (slow, rounded rising edges): add 4.7 kΩ to 3V3 on the trunk. Too
  strong (low level doesn't reach near 0 V): the trunk has too many in parallel.
  Fallback: `cargo run --release --features i2c-100k --bin t03_bus_soak`.

### t04 — T4 and T14: throughput and power

- **Purpose:** the firmware can drain nine FIFOs at 400 Hz without falling behind,
  and the 3.3 V rail carries the full load.
- **Run:** `cargo run --release --bin t04_throughput`, 10 minutes.
- **T14, while it runs:** meter in series between the board's 3V3 pin and the
  breadboard rail. Write down the current.
- **Pass:** no FIFO overflow; worst loop time under the 20 ms period; current a few
  tens of mA at most.

### t05 — T5: sample rate and saturation

- **Run:** `cargo run --release --bin t05_odr_sat`. First leave it still (rate
  check), then give the hardest knock you'd ever use right beside a module.
- **Pass:** each sensor reports 400 ± 2 % samples/s; knock peaks stay under 15.5 g.
- **If it saturates:** knock softer in the dataset, or note it for the report.

### t06 — T6: noise floor

- **Run:** start the capture script, then `cargo run --release --bin t06_noise`.
  Don't touch the table for 60 s.
- **Pass:** it prints RMS per sensor and a suggested 5σ trigger threshold. Record
  them. A sensor several times noisier than the rest is suspect.
- **Repeat** after the plate is assembled (M2); the mounted numbers are the ones
  the firmware will use.

### t07 — T7 and T8: knock capture (gain calibration, mounting comparison)

- **Run:** start the capture script with `--mode gain`, then
  `cargo run --release --bin t07_capture`.
- **T8 (before final assembly, on an offcut):** mount one module with
  cyanoacrylate and one with thin tape, the same distance from a marked knock
  point. 20 knocks. Pick the mounting with the higher and more repeatable peak.
- **T7 (assembled plate):** for each sensor, 20 medium knocks 2 cm from it, toward
  the plate center. The script computes gain c_i for each sensor. Then 20 knocks at
  the center for the symmetry check.
- **Pass:** after calibration, I1 ≈ I5, I3 ≈ I7 and the four corners agree within
  10 %. I1 ≠ I3 is expected: the plate is rectangular.

### t09 — T9: piezo front end

**Before connecting A0–A3, two checks:**

1. **Pin voltage limit.** The 1N4148 clamps let the pin reach about 3.95 V on a
   hard knock. Confirm in the STM32L475 datasheet (absolute maximum ratings, input
   voltage on pins in analog mode) that this is allowed. If it isn't, use 22 kΩ in
   series with 470 pF, or add a capacitor across each piezo, before going further.
2. **Meter checks, power on, A0–A3 not yet connected:**
   - VMID = 1.65 ± 0.05 V.
   - Each ADCj node reads about **1.5 V** on a 10 MΩ meter. That is the meter
     loading the 1 MΩ path, not a fault.
   - Power off, diode mode, in circuit: each clamp points the right way (band
     toward 3.3 V on the upper diode, band toward ADCj on the lower one).

- **Run:** capture script, then `cargo run --release --bin t09_piezo_dc`. Leave it
  idle 10 s, tap each disc in turn, then give one hard knock.
- **Pass:** idle ≈ 2048 counts on all four channels with low noise; each channel
  responds to its own disc; on the hard knock the waveform flat-tops at the clamp
  level and the board keeps running.

### t10 — T10 and T11: piezo timing and ring-down

- **Run:** capture script, then `cargo run --release --bin t10_piezo_time`.
  - Knock directly beside P0, then P1, P2, P3.
  - Knock at the plate center, 10 times.
  - For T11, run with `--features long-window` and knock a few times anywhere.
- **Pass:** the nearest piezo fires first every time; center knocks give arrival
  times within about 0.1 ms of each other; the script prints an estimate of
  v_eff. For T11, read off the time for the tail to fall below the trigger
  threshold: that sets the refractory period (expected around 300 ms).

### t13 — T13: soak and recovery

- **Run:** `cargo run --release --bin t13_recovery`, 30 minutes. During the run,
  unplug one sensor's SDA wire, wait 5 s, plug it back. Repeat with a power wire.
- **Pass:** the log reports the sensor lost, the bus recovery, and the sensor
  back; other sensors keep reporting; the firmware never hangs.

### T12 — latency and false triggers (later)

Needs the real acquisition firmware and the avatar, so there is no test binary yet.
Method when ready: film the knock and the screen with a phone at 240 fps and count
frames (≈ 4 ms each); count triggers over 10 minutes idle, 10 minutes with someone
typing on the table, and with deliberate table bumps.

---

## 4. Recording results

Put numbers in [CHECKLIST.md](CHECKLIST.md). Captured CSV files go in `data/bench/`
(git-ignored); mention the file name next to the checklist line.
