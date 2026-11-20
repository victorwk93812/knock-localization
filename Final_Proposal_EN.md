# Knock Localization on an Acrylic Board — Final Proposal

**Platform:** STM32L475VG (B-L475E-IOT01A Discovery kit) · 9 accelerometers · 4 piezo discs · Python host rendering

---

## 1. Overview

We detect **where a knock lands on a 45 × 45 cm acrylic board** from the vibration it produces, and use the result to drive an interactive display on a laptop.

The project is evaluated at two levels:

| | Benchmark 1 — Zone classification | Benchmark 2 — Continuous localization |
|---|---|---|
| Question | Which of the 3 × 3 cells was knocked? | What is the knock position (x, y) in cm? |
| Principle | Nearest sensor (largest vibration amplitude) | Amplitude model, piezo arrival times, and data-driven regression |
| Output | Discrete: one of 9 zones → a preset avatar action | Continuous: a point on the board → ripple plus avatar gaze/pointing |
| Computed on | STM32 (real time) | Laptop (Python), with the best simple method optionally ported to the STM32 |

**Both benchmarks share the same hardware, firmware acquisition pipeline, and recorded knock dataset**, so their results are directly comparable. Benchmark 1 is the baseline that Benchmark 2 must beat (see §7 for the quantitative criterion).

---

## 2. Board layout

The board is 45 × 45 cm with 15 cm cells. Coordinates are in cm, with the origin at the board center.

```
  y
  ↑  ┌─────────────┬─────────────┬─────────────┐
     │     I0      │     I1      │     I2      │   I0 (−15, 15)  I1 (0, 15)   I2 (15, 15)
     │             │             │             │
     ├──────────P0─┼─────────────┼─P1──────────┤   P0..P3 at (±7.5, ±7.5)
     │     I7      │  STM32 + I8 │     I3      │   I7 (−15, 0)   I8 (0, 0)    I3 (15, 0)
     ├──────────P3─┼─────────────┼─P2──────────┤
     │     I6      │     I5      │     I4      │   I6 (−15,−15)  I5 (0,−15)   I4 (15,−15)
     │             │             │             │
     └─────────────┴─────────────┴─────────────┘ → x
```

- **I0–I7:** external IMUs at the centers of the 8 outer cells.
- **I8:** the Discovery board's onboard LSM6DSL at the center cell. Its exact position on the PCB is offset from the board center and must be measured. Using it adds a **9th zone**, the center, which the original 8-zone plan could not detect.
- **P0–P3:** piezo discs at the four inner grid intersections, away from the board edges to reduce reflections.
- **Zone ID = sensor ID** (0–8).
- **Grid marking:** because the acrylic is transparent, print a paper template showing the cell lines and the 6 cm test grid (§7), and tape it under the board. The same template provides the labeled knock points for the Benchmark 2 dataset.
- **Sensor orientation:** all IMUs are mounted with the same orientation (silkscreen +x arrow pointing the same way), so their axes agree.

---

## 3. Hardware

### 3.1 Bill of materials

| Item | Qty | Purpose / notes |
|---|---|---|
| B-L475E-IOT01A Discovery kit | 1 | MCU, onboard IMU (I8), ST-LINK virtual COM port, Wi-Fi |
| Acrylic sheet, 5 mm, 45 × 45 cm | 1 | Sensing surface (standard cut; drilling optional) |
| IMU modules (LSM6DSOX/LSM6DSO preferred; LSM6DS3TR-C/LSM6DS3, BMI160 or ADXL345 acceptable), **all the same model** | 8 + 2 spare | External sensors I0–I7; must use I2C and run on 3.3 V |
| TCA9548A I2C multiplexer module | 1 + 1 spare | 8 channels → one IMU per channel; resolves the identical-address conflict |
| Dupont wire, female–female, 30 cm | ~40 | IMU spokes (center to corner ≈ 21 cm, plus slack) |
| Dupont wire, male–male / male–female, 10–20 cm | ~20 | Breadboard connections |
| Half-size breadboard | 1 | Mux and piezo front end (solderless) |
| Piezo discs, 27 mm, with leads | 4 + 2 spare | High-bandwidth vibration sensing (timing and amplitude) |
| Resistors: 1 MΩ, 10 kΩ, 100 kΩ | 6, 6, 4 | Piezo front end |
| Capacitors: 10 µF, 100 nF, 1 nF | 2, 4, 6 | Mid-rail bias decoupling, anti-alias filter |
| Schottky diodes BAT43 / BAT85 (through-hole) | 10 | ADC input clamps (2 per channel); avoid high-leakage 1N5819 |
| Male pin headers (40-pin) | 2 | Spares |
| Cyanoacrylate glue, Kapton or electrical tape, cable ties/clips, thin double-sided tape | — | Mounting and cable management |
| EVA foam feet | 4–6 | Isolate the board from the table |
| Micro-USB cable (data-capable) | 1 (+1 spare) | ST-LINK: power, flashing, virtual COM port |
| USB logic analyzer, 8-ch 24 MHz (recommended) | 1 | I2C debugging and timing measurements |
| Soldering iron, solder, heat shrink (if needed) | — | Module headers, piezo leads |

### 3.2 Wiring

**IMUs**
- STM32 3.3 V / GND → TCA9548A VCC / GND.
- STM32 Arduino header **D14 (SDA) / D15 (SCL)** (I2C1) → TCA9548A SDA / SCL. *Verify the pin mapping in user manual UM2153.*
- TCA9548A channel *k* (SDk / SCk) → IMU *k* SDA / SCL, for k = 0…7. IMU VCC / GND go to the 3.3 V / GND rails.
- With one IMU per mux channel, each channel sees only that module's pull-up resistors, so no extra pull-ups are needed.
- The onboard I8 sits on the board's internal I2C2 bus, which is already wired and independent of I2C1.

**Piezo front end** (built on the breadboard; one shared bias plus 4 identical channels)

- **Mid-rail bias VMID ≈ 1.65 V (shared):** 100 kΩ from 3.3 V to VMID, 100 kΩ from VMID to GND, and 10 µF ∥ 100 nF from VMID to GND.
- **Each channel j = 0…3:**
  - Piezo terminal A → VMID; piezo terminal B → node Pj.
  - 1 MΩ between Pj and VMID (bleeds charge, sets the DC level).
  - 10 kΩ from Pj to node ADCj (current limit).
  - BAT43 from ADCj to 3.3 V (cathode at 3.3 V) and BAT43 from GND to ADCj (cathode at ADCj), clamping the pin to its safe range.
  - 1 nF from ADCj to GND (anti-alias filter, corner ≈ 16 kHz).
  - ADCj → Arduino **A0–A3**.

A hard knock can produce tens of volts at the piezo; the series resistor and clamps keep the ADC pin within 0–3.3 V. Because of the 10 kΩ source impedance, the ADC sampling time is set long enough to settle.

### 3.3 Mounting

Rule: **sensors rigidly coupled to the board; board isolated from the table.**

- **IMUs:** bond each module flat to the board with a thin layer of cyanoacrylate. Avoid foam tape, which damps exactly the vibration we measure. (If the board is drilled, M2.5 screws with thin washers also work, but not standoffs.)
- **Piezos:** glue the brass face directly to the board with thin cyanoacrylate. Use pre-leaded discs, or solder the leads before gluing.
- **Discovery board (center):** its IMU is sensor I8, so it must be firmly attached. Use short rigid spacers (≤ 5 mm) screwed tight if the board is drilled; otherwise use thin double-sided tape plus hot glue at the edges. Its coupling will differ from the glued IMUs, which is handled by per-channel gain calibration (test T7).
- **Cables:** tape every cable flat along its route and add strain relief at the breadboard and USB connector. A cable tapping the board produces fake knocks.
- **Feet:** place EVA pads slightly inward from the corners.

---

## 4. Firmware: shared acquisition pipeline

### 4.1 Sensor configuration and bus budget

- Accelerometer only (gyroscope off), **±16 g** full scale (knocks saturate ±2 g), **ODR 416 Hz**, FIFO in continuous mode.
- **Why 416 Hz:** all 8 external IMUs share one 400 kHz I2C bus through the mux. At 416 Hz, 8 IMUs × 416 samples/s × 7 bytes (FIFO word including tag) ≈ 23 kB/s ≈ 210 kbit/s, which is about 55 % bus load before addressing overhead. 833 Hz would exceed the bus capacity. **High-frequency information comes from the piezos.**
- **Round-robin readout:** every 10 ms, for k = 0…7, select mux channel k, read the FIFO level, and burst-read the pending samples (~4 per IMU). I8 is read on I2C2 in parallel.
- **Timestamps:** the MCU timer stamps each burst; individual sample times are reconstructed from the ODR spacing (and from FIFO timestamps if the chip supports them).
- **Piezos:** the ADC scans the 4 channels at **~100 kS/s per channel** into a circular DMA buffer (~50 ms). The known inter-channel scan skew is corrected in software.

### 4.2 Knock event detection

- **Trigger:** any piezo deviating from VMID by more than a threshold. If the piezo front end is not built, the fallback is any IMU with |a| − 1 g above a threshold.
- **Windows:** IMU data from −5 to +40 ms around the trigger; piezo data from −1 to +10 ms.
- **Debounce / refractory period:** ~300 ms, tuned to the board's measured ring-down (test T11).

### 4.3 Features per knock

- **IMU i (0–8):** peak dynamic acceleration A_i = max_t |a_i(t) − g_i|, where g_i is the gravity baseline from the pre-trigger samples. Optionally also per-axis peaks (especially z, normal to the board) and window energy. Each A_i is scaled by its calibrated gain c_i (test T7).
- **Piezo j (0–3):** onset time t_j (first crossing of k·σ_noise, or cross-correlation), peak amplitude, and energy.

### 4.4 Output to the host

Over the ST-LINK virtual COM port (USART1) at **921 600 baud** (verify that the ST-LINK supports this rate; otherwise use the highest stable rate):

- **Live mode:** for each knock, one line `HIT:<zone>` (Benchmark 1 result, computed on the MCU), plus one feature line `FEAT:<n>,<A0..A8>,<t0..t3>,<P0..P3>` (input for Benchmark 2 on the laptop).
- **Collection mode:** dumps the raw windows (IMU and piezo samples) for every knock, for offline analysis and training. Latency does not matter in this mode.
- **Optional:** the same lines over the onboard es-WiFi module as a TCP socket (e.g. port 8002) for a wireless demo. Use a phone hotspot, because campus Wi-Fi often isolates clients.

---

## 5. Benchmark 1 — Zone classification (nearest-sensor baseline)

### 5.1 Method (on the STM32)

- zone = argmax_i (c_i · A_i), for i = 0…8.
- **Crosstalk guard:** accept the result only if the largest value is at least r times the second largest (r ≈ 1.2–1.5, tuned on data). Otherwise report "uncertain" and trigger no action.
- The original plan also used "first to rise" as a criterion. At a 416 Hz IMU rate, the arrival-time differences across the board are below one sample period, so Benchmark 1 uses **amplitude only**. Timing is handled by the piezos in Benchmark 2.

### 5.2 Host: avatar actions

A Python script (`pyserial` + `pygame`) reads `HIT:<zone>` and drives an avatar state machine: **Idle (breathing loop) → Action → back to Idle**.

| Zone | Position | Action |
|---|---|---|
| 0 | top-left | Wave hello |
| 1 | top-center | Nod in agreement |
| 2 | top-right | Clap and cheer |
| 3 | right | Turn to the right |
| 4 | bottom-right | Hands on head, surprised |
| 5 | bottom-center | Laugh |
| 6 | bottom-left | Bow in thanks |
| 7 | left | Turn to the left |
| 8 | center (new) | Jump |

Assets: 9 sprite sequences (or GIFs or 3D animation clips) plus an idle loop.

---

## 6. Benchmark 2 — Continuous (x, y) localization

Benchmark 2 is a ladder of methods, all evaluated on the same dataset. Each step should beat the previous one.

**2a. Amplitude-weighted centroid** (simplest continuous estimate, runs on the MCU)
(x, y) = Σ w_i r_i / Σ w_i, with w_i = (c_i A_i)^p and the exponent p tuned on data. No physical model is involved.

**2b. Physical amplitude model**
A_i = S · ρ_i^(−1/2) · e^(−α ρ_i), where ρ_i = √(|r − r_i|² + ρ₀²).
This combines cylindrical spreading with damping; ρ₀ regularizes the near field. The unknowns are (x, y, S); α and ρ₀ are fitted once on calibration data. The fit is nonlinear least squares in log space (`scipy.optimize.least_squares`). The unknown knock strength S drops out of the amplitude ratios.

**2c. Piezo time-difference of arrival (TDOA)**
t_j = t₀ + |r − p_j| / v_eff, with unknowns (x, y, t₀), solved by least squares. Bending waves are **dispersive** (c ∝ √f). For 5 mm acrylic, rough estimates are c ~ 40 m/s at 100 Hz and ~130 m/s at 1 kHz, so the piezo signals are band-pass filtered to a fixed band and v_eff is calibrated from knocks at known positions.

**2d. Data-driven regression**
Features such as log-amplitude ratios log(A_i / ΣA), piezo time differences and piezo amplitudes are mapped to (x, y) with k-NN regression, Gaussian-process regression, or a small MLP (`scikit-learn`), trained on labeled grid knocks.

**Stretch goals:** feed the 2b/2c estimates into 2d as extra features; port the best lightweight model (e.g. k-NN or 2a) to the STM32.

### 6.1 Physics discussion (central analysis point of the report)

In the IMU bandwidth (≲ 200 Hz), bending wavelengths in 5 mm acrylic are about 0.3–0.4 m, comparable to the board itself. The response is therefore dominated by **plate modes** rather than outward-propagating waves. We expect the simple decay model (2b) to be biased in this regime, while data-driven regression (2d) can learn the position-dependent mode-shape "fingerprints". The piezos see kHz content, where wavelengths are about 10 cm and the board behaves more like a propagating medium, which favors TDOA (2c). Showing which method wins in which regime, and where on the board (edges, near supports), is the main analytical result.

### 6.2 Host visualization

- A map of the board with a ripple animation at the estimated (x, y), an uncertainty circle, and a live error heat map during evaluation.
- The **avatar turns its head and points toward the knock location**. This is continuous control, which bridges Benchmark 1's discrete actions and Benchmark 2's continuous output.

---

## 7. Evaluation protocol

### 7.1 Dataset
- **Test grid:** 7 × 7 points, x, y ∈ {−18, −12, −6, 0, 6, 12, 18} cm (49 points, printed on the template under the board).
- **15 knocks per point**, by at least 2 people, at medium force with a fingertip or knuckle, giving ≈ 735 knocks recorded in collection mode (raw data).
- **Robustness sets:** soft vs. hard knocks; pen vs. knuckle; an object resting on the board; a person not in the training set.
- All methods are evaluated **offline on identical recordings**, and the best ones are then run live.

### 7.2 Splits
1. Random split by knock (positions seen during training).
2. **Leave-points-out** (positions never seen during training), the honest test of continuous localization.

### 7.3 Metrics
- **Benchmark 1:** 9-class accuracy and confusion matrix. Errors are also expressed in cm by assigning each predicted zone its cell center, so they can be compared with Benchmark 2.
- **Benchmark 2:** mean, median and 90th-percentile Euclidean error (cm); an error heat map over the board; and a derived zone accuracy (each (x, y) mapped to its cell) for direct comparison with Benchmark 1.
- **System:** end-to-end latency from knock to avatar reaction, measured with phone slow-motion video (240 fps ≈ 4 ms resolution); false triggers per 10 minutes, both idle and with someone typing on the same table.

### 7.4 Success criteria
- **Benchmark 1:** ≥ 95 % zone accuracy for knocks near cell centers.
- **Benchmark 1's quantization floor:** even with perfect zone classification, reporting the cell center gives a mean error of ≈ 0.38 × 15 cm ≈ **5.7 cm** (the mean distance from a uniformly random point in a square to its center). **Benchmark 2 is meaningful only if it clearly beats this. Target: median error ≤ 3 cm.**
- **Latency:** ≤ 100 ms from knock to on-screen reaction.

---

## 8. Test and verification plan

| ID | Test | Pass criterion |
|---|---|---|
| T1 | **Incoming module check:** each IMU alone on the breadboard at 3.3 V; read WHO_AM_I; measure current; label modules 0–9 | All respond with the correct ID |
| T2 | **Multiplexer:** write `1 << k` to 0x70 and read it back; I2C scan on every channel | Exactly one IMU found on each of channels 0–7 |
| T3 | **Bus integrity:** all 8 IMUs on 30 cm wires at 400 kHz; 10⁶ reads (~30 min); check edges with the logic analyzer | 0 errors (fallback: 100 kHz or stronger pull-ups) |
| T4 | **Throughput and timing:** measure the round-robin loop time (GPIO toggle + logic analyzer, or DWT cycle counter); watch FIFO levels for 10 min | No FIFO overflow at 416 Hz |
| T5 | **Configuration and saturation:** count samples per second for each IMU; hardest knock right next to an IMU | ODR correct; peaks stay below ±16 g |
| T6 | **Noise floor:** each IMU stationary for 60 s; compute RMS | Thresholds set to ≥ 5σ |
| T7 | **Gain calibration:** for each IMU, 20 knocks at 5 cm from it toward the board center → gain c_i. Symmetry check: a center knock should give equal amplitudes on the 4 edge IMUs and on the 4 corner IMUs | Spread after calibration < 10 % |
| T8 | **Mounting comparison** (on an acrylic offcut, before final assembly): glue vs. thin tape vs. hot glue | Pick the method with the highest, most repeatable amplitude |
| T9 | **Piezo front end:** measure VMID with the multimeter; check the idle ADC level; record the hardest knock | VMID = 1.65 ± 0.05 V; ADC idles at ≈ mid-scale; clamps hold, no damage |
| T10 | **Piezo timing sanity:** knock on top of a piezo, then at the center | The nearest piezo fires first; a center knock gives symmetric arrivals; v_eff estimated |
| T11 | **Ring-down:** record knock tails | Sets the debounce period |
| T12 | **End-to-end latency and false triggers** (§7.3) | ≤ 100 ms; ~0 false triggers per 10 min |
| T13 | **Soak and recovery:** 30 min continuous operation; unplug and replug one IMU cable | No hang; the I2C bus-recovery routine (9 SCL pulses + re-init) restores operation |
| T14 | **Power:** total 3.3 V current with all sensors (multimeter in series) | Well within the board's regulator budget |

---

## 9. Milestones and roles

| Milestone | Content | Owner role |
|---|---|---|
| M1 | Electronics bring-up on the breadboard (T1–T6, T9) | Electronics |
| M2 | Mounting test (T8), board assembly, T7, T10, T11 | Electronics + Board & assembly |
| M3 | Firmware acquisition pipeline and collection mode; record the dataset | Firmware |
| M4 | Benchmark 1 live demo with avatar | Firmware + Host software |
| M5 | Benchmark 2 methods offline; best method live | Host software / analysis |
| M6 | Evaluation (§7), report, demo rehearsal (T12, T13) | All |

---

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| I2C bandwidth caps the IMU rate at ~416 Hz | Piezos supply the high-frequency information; if IMU amplitudes prove insufficient, move to SPI (6 wires per IMU) or split the IMUs across two I2C buses |
| LSM6DSOX not available locally | Use another single model (LSM6DS3TR-C / BMI160 / ADXL345); never mix models; adapt the FIFO driver |
| Plate modes and edge effects bias the physical models | Data-driven regression (2d) plus calibration; report the regime dependence (§6.1) |
| The center board couples differently from the glued IMUs | Per-channel gain calibration (T7) |
| Cable movement causes fake knocks | Tape cables flat, add strain relief, use the crosstalk guard |
| Campus Wi-Fi isolates clients | USB virtual COM port as the default link; phone hotspot for the Wi-Fi demo |
