# Knock Localization on an Acrylic Plate — Final Proposal (v2)

**Platform:** STM32L475VG (B-L475E-IOT01A Discovery kit) · 9 × BMI160 accelerometers · 4 × piezo discs · Python host rendering
**Sensing surface:** acrylic sheet, **30 × 20 cm, 2 mm thick**

> **What changed from v1:** smaller and thinner plate (30 × 20 × 0.2 cm instead of 45 × 45 × 0.5 cm); the bill of materials now lists the parts actually bought (GY-BMI160 modules, 1N4148 clamps, Φ27 piezo discs); the Discovery board moves **off** the plate, and a 9th BMI160 takes over the center zone; all geometry, bus budget, physics estimates and success criteria are recomputed for the new plate.

---

## 1. Overview

We detect **where a knock lands on a 30 × 20 cm acrylic plate** from the vibration it produces, and use the result to drive an interactive display on a laptop.

| | Benchmark 1 — Zone classification | Benchmark 2 — Continuous localization |
|---|---|---|
| Question | Which of the 3 × 3 cells was knocked? | What is the knock position (x, y) in cm? |
| Principle | Nearest sensor (largest vibration amplitude) | Amplitude model, piezo arrival times, and data-driven regression |
| Output | Discrete: one of 9 zones → a preset avatar action | Continuous: a point on the plate → ripple plus avatar gaze/pointing |
| Computed on | STM32 (real time) | Laptop (Python); the best simple method optionally ported to the STM32 |

**Both benchmarks share the same hardware, firmware acquisition pipeline and recorded knock dataset**, so their results are directly comparable. Benchmark 1 is the baseline Benchmark 2 must beat (quantitative criterion in §7.4).

---

## 2. Plate layout

The plate is 30 cm (x) × 20 cm (y), divided into a 3 × 3 grid of **10 × 6.67 cm** cells. Coordinates are in cm, origin at the plate center.

```
  y
  ↑  ┌──────────────┬──────────────┬──────────────┐  +10
     │      I0      │      I1      │      I2      │       I0 (−10, 6.67)  I1 (0, 6.67)  I2 (10, 6.67)
     │              │              │              │
     ├───────────P0─┼──────────────┼─P1───────────┤ +3.33 P0..P3 at (±5, ±3.33)
     │      I7      │      I8      │      I3      │       I7 (−10, 0)     I8 (0, 0)     I3 (10, 0)
     ├───────────P3─┼──────────────┼─P2───────────┤ −3.33
     │      I6      │      I5      │      I4      │       I6 (−10,−6.67)  I5 (0,−6.67)  I4 (10,−6.67)
     │              │              │              │
     └──────────────┴──────────────┴──────────────┘  −10
   −15            −5               5             15 → x

        ════ cable bundle ════►  [ Discovery board + breadboard, on the table beside the plate ]
```

- **I0–I8:** nine GY-BMI160 modules at the centers of the nine cells. **Zone ID = sensor ID** (0–8).
- **P0–P3:** Φ27 piezo discs at the four inner grid intersections.
- **Electronics off the plate.** The plate itself weighs only about **140 g**. The Discovery board plus breadboard would add a comparable mass, cover most of the center cell and dominate the plate's vibration modes. They sit on the table next to the plate instead, and all sensor cables leave from one edge.
- **Table reference sensor (new):** the Discovery board's onboard LSM6DSL now sits on the table, so it measures **table-borne** vibration. It is used to reject false triggers (someone bumping the table), see §4.2.
- **Grid marking:** print a 1:1 paper template (cell lines + the test grid of §7.1) and tape it under the transparent plate. The same template provides the labeled knock points for the Benchmark 2 dataset.
- **Sensor orientation:** all nine modules glued with the same orientation (silkscreen axes pointing the same way).

---

## 3. Hardware

### 3.1 Bill of materials (as purchased)

✔ = bought at 光華商場 on 2026-11-01 · ☐ = still to confirm / borrow. *Please check quantities against the receipts.*

| Item | Qty | Status | Purpose / notes |
|---|---|---|---|
| B-L475E-IOT01A Discovery kit | 1 | ✔ (owned) | MCU, ST-LINK virtual COM port, onboard LSM6DSL (table reference), Wi-Fi |
| Acrylic sheet, **30 × 20 cm, 2 mm** | 1 | ✔ | Sensing surface; **do not drill** (2 mm acrylic cracks easily), everything is glued |
| **GY-BMI160** 6-DoF modules (今華 JIN-HUA, NT$95 each) | 10 (9 + 1 spare) | ✔ | Sensors I0–I8; accelerometer ±16 g, up to 1.6 kHz ODR, 1 KB FIFO, sensor-time counter |
| TCA9548A I2C multiplexer module | 1 (+1 spare) | ✔ | 8 channels for I0–I7 (all at address 0x68) |
| Dupont wire, female–female, 30 cm | ~40 | ✔ | Sensor cables (plate → off-board breadboard) |
| Dupont wire, male–male / male–female, 10–20 cm | ~20 | ✔ (+ owned) | Breadboard connections |
| Half-size breadboard | 1 | ✔ | TCA9548A + piezo front end, next to the Discovery board |
| Piezo discs, **Φ27 mm** bare brass (扁平壓電式蜂鳴器, 2 per pack) | 6 (4 + 2 spare) | ✔ | High-bandwidth sensing: arrival times and amplitudes |
| Stranded wire **26 AWG, red + black**, ~3 m each | 6 m | ✔ | Piezo leads (red = silver ceramic, black = brass), twisted pairs |
| Metal-film resistors: 1 MΩ, 10 kΩ, 100 kΩ | 6, 6, 4 | ✔ | Piezo front end |
| Capacitors: 10 µF electrolytic (16–25 V), 100 nF ceramic (104), 1 nF ceramic (102) | 2, 4, 6 | ✔ | Mid-rail bias decoupling, anti-alias filter |
| **1N4148** switching diodes, through-hole | 10 (8 used) | ✔ | ADC input clamps (replaces BAT43: low leakage, ~0.65 V clamp) |
| 1 × 40 male pin headers, 2.54 mm | 2 strips | ✔ | Spares / breadboard adapters |
| Nylon cable ties **CV-150** (150 × 3.6 mm) | 100 | ✔ | Bundling cable spokes |
| Cyanoacrylate glue (三秒膠), Kapton / electrical tape, thin double-sided tape | — | ✔ | Mounting, cable fixing |
| EVA foam feet | 4 | ✔ / ☐ | Isolate the plate from the table |
| Micro-USB cable (data-capable) | 1 | ✔ (owned) | Power, flashing, virtual COM port |
| Multimeter | 1 | ✔ (owned) | Bias, continuity, current tests |
| Soldering iron, solder, heat shrink, flush cutters | — | ☐ | Module headers (10 × ~8 pins), piezo leads |
| USB logic analyzer, 8-ch 24 MHz | 1 | ☐ (recommended) | I2C debugging and timing |

Dropped from v1: M2.5 screws, standoffs and laser-cut holes (not usable on a 2 mm sheet), BAT43/BAT85 (not stocked).

### 3.2 Module preparation (GY-BMI160)

- **CS → 3.3 V.** If CS is low at power-up the BMI160 starts in SPI mode and never answers on I2C.
- **SAO (SDO) → GND** on I0–I7 (address **0x68**); **SAO → 3.3 V on I8** (address **0x69**).
- Make both straps **on the module itself** with a short wire link, so only 4 wires (VCC, GND, SCL, SDA) run to each sensor.
- Solder the header from the component side and **trim the pin tails flush underneath**, so the back of the PCB sits flat on the acrylic. (Or solder the four wires directly and skip the header.)
- Power through the module's VIN/3V3 pin from the Discovery board's 3.3 V rail; check with the multimeter that the chip sees ≈3.3 V (some modules have an onboard regulator with dropout).

### 3.3 Wiring

**Accelerometers**
- Discovery 3.3 V / GND → breadboard power rails → TCA9548A VCC / GND.
- Discovery Arduino header **D14 (SDA) / D15 (SCL)** (I2C1) → TCA9548A upstream SDA / SCL. *Verify the pin mapping in user manual UM2153.*
- TCA9548A channel *k* (SDk / SCk) → I*k* SDA / SCL, k = 0…7.
- **I8 (0x69)** connects to the **upstream** I2C1 trunk, next to the TCA9548A (0x70). Whatever mux channel is open, the visible addresses are 0x68 (channel IMU), 0x69 (I8), 0x70 (mux), so there is no conflict.
- The modules carry their own pull-ups, so no extra pull-ups at first; see test T3 for the fallback.
- The onboard LSM6DSL (table reference) is on the board's internal I2C2 bus, independent of I2C1.

**Piezo front end** (on the breadboard; one shared bias + 4 identical channels)
- **Mid-rail bias VMID ≈ 1.65 V (shared):** 100 kΩ from 3.3 V to VMID, 100 kΩ from VMID to GND, 10 µF (+ to VMID) ∥ 100 nF from VMID to GND.
- **Each channel j = 0…3:**
  - Piezo black (brass) → VMID; piezo red (ceramic) → node Pj.
  - 1 MΩ between Pj and VMID (bleeds charge, sets the DC level).
  - 10 kΩ from Pj to node ADCj (current limit).
  - 1N4148 from ADCj to 3.3 V (**band toward 3.3 V**) and 1N4148 from GND to ADCj (**band toward ADCj**).
  - 1 nF from ADCj to GND (anti-alias, corner ≈ 16 kHz).
  - ADCj → Arduino **A0–A3**.
- With 1N4148 the pin is clamped to about −0.65 V … 3.95 V. The 10 kΩ limits the injected current to a few mA even for a ~40 V spike, within the STM32's injected-current rating. If test T9 shows larger spikes, raise the series resistor to 22 kΩ and the filter capacitor drops to 470 pF to keep the same corner.
- 1N4148 leakage at 1.65 V is only a few nA, so the shift across the 1 MΩ is a few mV (power Schottkys like 1N5819 would shift it by hundreds of mV).
- Set the ADC sampling time long enough for the 10 kΩ source impedance.

### 3.4 Mounting

Rule: **sensors rigidly coupled to the plate; plate isolated from the table; as little extra mass on the plate as possible.**

- **BMI160 modules:** thin layer of cyanoacrylate under the flush-trimmed PCB. No foam tape (it damps exactly what we measure). Cyanoacrylate may leave a white haze on acrylic; this is cosmetic only.
- **Piezos:** brass face glued directly to the plate with a thin layer of cyanoacrylate. Solder the leads first: brass rim (black), silver ceramic (red, quick and light so the silver does not lift).
- **Cables:** route all spokes toward one long edge, tape them flat, bundle with CV-150 ties **off** the plate, and add strain relief at the plate edge and the breadboard. A cable slapping the plate is a fake knock.
- **Feet:** four EVA pads about 2 cm in from each corner.
- **Handling:** 2 mm acrylic flexes and can crack; knock with fingertip or knuckle, and use pens only gently for the robustness set.

---

## 4. Firmware: shared acquisition pipeline

### 4.1 Sensor configuration and bus budget

- **BMI160:** accelerometer only (gyroscope suspended), **±16 g**, **ODR 400 Hz** (normal filter mode), FIFO accelerometer-only and headerless (6 bytes/frame). Key registers (verify in the datasheet): CHIP_ID 0x00 = 0xD1; CMD 0x7E ← 0x11 (accel normal mode); ACC_CONF 0x40; ACC_RANGE 0x41 ← 0x0C (±16 g); FIFO_CONFIG_1 0x47; FIFO_LENGTH 0x22–0x23; FIFO_DATA 0x24; SENSORTIME 0x18–0x1A (39 µs resolution).
- **Why 400 Hz:** all nine BMI160s share one 400 kHz I2C bus. 9 × 400 frames/s × 6 B ≈ 21.6 kB/s of data; with mux writes, FIFO-length reads and addressing it is ≈ 60–70 % bus load. 800 Hz would exceed it. The TCA9548A is limited to 400 kHz, so Fast-mode Plus is not an option. **High-frequency information comes from the piezos.**
- **Round-robin readout every 20 ms:** for k = 0…7 select mux channel k, read FIFO_LENGTH and burst-read ~8 frames; read I8 the same way. The 1 KB FIFO (~170 frames ≈ 425 ms) gives plenty of margin.
- **Timestamps:** each burst is stamped with an MCU timer and the BMI160 sensor time; sample times are reconstructed from the ODR spacing.
- **Table reference (LSM6DSL):** 416 Hz on I2C2, same windowing.
- **Piezos:** ADC scans 4 channels at **~100 kS/s per channel** into a circular DMA buffer (~50 ms); the known inter-channel scan skew is corrected in software (or use simultaneous dual-ADC mode).

### 4.2 Knock event detection

- **Trigger:** any piezo deviating from VMID by more than a threshold; fallback without piezos: any BMI160 with |a| − 1 g above a threshold.
- **Table-bump rejection:** if the table reference amplitude exceeds a fraction of the largest plate amplitude (tuned in T12), the event is rejected.
- **Windows:** accelerometers −10 … +40 ms around the trigger; piezos −1 … +10 ms.
- **Refractory period:** ~300 ms, tuned to the measured ring-down (T11).

### 4.3 Features per knock

- **Accelerometer i (0–8):** peak dynamic acceleration A_i = max_t |a_i(t) − g_i| (g_i = pre-trigger gravity baseline), plus z-axis peak and window energy; scaled by calibrated gain c_i (T7).
- **Piezo j (0–3):** onset time t_j (first k·σ crossing or cross-correlation), peak amplitude, band energy.

### 4.4 Output to the host

ST-LINK virtual COM port (USART1), 921 600 baud if stable, otherwise the highest stable rate:

- **Live mode:** `HIT:<zone>` (Benchmark 1, computed on the MCU) and `FEAT:<n>,<A0..A8>,<t0..t3>,<P0..P3>` (input to Benchmark 2).
- **Collection mode:** raw windows for every knock, for offline analysis and training.
- **Optional:** same lines over the onboard es-WiFi module (TCP, e.g. port 8002) via a phone hotspot.

---

## 5. Benchmark 1 — Zone classification (nearest-sensor baseline)

### 5.1 Method (on the STM32)

- zone = argmax_i (c_i · A_i), i = 0…8.
- **Crosstalk guard:** accept only if the largest value is ≥ r × the second largest (r ≈ 1.2–1.5, tuned on data); otherwise report "uncertain" and play nothing. On the smaller plate neighboring sensors are only 6.7–10 cm apart, so this guard matters more than in v1.
- Amplitude only: at 400 Hz the arrival-time differences across the plate are below one sample period; timing is left to the piezos (Benchmark 2).

### 5.2 Host: avatar actions

Python (`pyserial` + `pygame`) reads `HIT:<zone>` and drives an avatar state machine **Idle (breathing loop) → Action → Idle**.

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
| 8 | center | Jump |

Assets: 9 sprite sequences (or GIFs / 3D clips) plus an idle loop.

---

## 6. Benchmark 2 — Continuous (x, y) localization

A ladder of methods, all evaluated on the same dataset; each step should beat the previous one.

**2a. Amplitude-weighted centroid** (runs on the MCU): (x, y) = Σ w_i r_i / Σ w_i, w_i = (c_i A_i)^p, p tuned on data.

**2b. Physical amplitude model:** A_i = S · ρ_i^(−1/2) · e^(−α ρ_i), ρ_i = √(|r − r_i|² + ρ₀²). Unknowns (x, y, S); α, ρ₀ fitted once on calibration data; nonlinear least squares in log space (`scipy.optimize.least_squares`). S cancels in amplitude ratios.

**2c. Piezo TDOA:** t_j = t₀ + |r − p_j| / v_eff, unknowns (x, y, t₀). Bending waves are dispersive (c ∝ √f). For 2 mm acrylic, estimated c ≈ 25 m/s at 100 Hz, ≈ 80 m/s at 1 kHz, ≈ 180 m/s at 5 kHz. Across 10 cm of piezo spacing that is ~0.5–1 ms of delay; at 100 kS/s one sample (10 µs) corresponds to roughly 1–2 mm, so the ADC timing is not the limit. Piezo signals are band-pass filtered to a fixed band (e.g. 2–8 kHz) and v_eff is calibrated from knocks at known positions.

**2d. Data-driven regression:** log-amplitude ratios log(A_i/ΣA), piezo time differences and amplitudes → (x, y) with k-NN, Gaussian-process regression or a small MLP (`scikit-learn`), trained on labeled grid knocks.

**Stretch goals:** feed 2b/2c estimates into 2d as features; port the best lightweight model (k-NN or 2a) to the STM32.

### 6.1 Physics discussion (central analysis of the report)

The thinner plate changes the regime. For 2 mm acrylic (E ≈ 3.2 GPa, ρ ≈ 1190 kg/m³) the bending stiffness is about 16 × lower than at 5 mm, so waves are slower and shorter:

| Frequency | Bending-wave speed | Wavelength |
|---|---|---|
| 100 Hz | ≈ 25 m/s | ≈ 25 cm |
| 200 Hz | ≈ 36 m/s | ≈ 18 cm |
| 1 kHz | ≈ 80 m/s | ≈ 8 cm |
| 5 kHz | ≈ 180 m/s | ≈ 3.6 cm |

In the accelerometer band (≲ 200 Hz at 400 Hz ODR) the wavelength is comparable to the plate (30 × 20 cm), and only about **six plate modes** exist below 200 Hz. The accelerometer response is therefore a superposition of a few mode shapes rather than an outgoing wave, so the simple decay model (2b) is expected to be biased while regression (2d) can learn the mode-shape "fingerprint". In the piezo band (kHz) the wavelength drops to a few cm, the plate behaves as a propagating medium and TDOA (2c) should work best, apart from edge reflections, which arrive quickly on a small plate (hence short onset-detection windows). The foam feet and the cable bundle perturb the modes; mapping where each method wins (center vs. edges vs. near the feet) is the main analytical result.

### 6.2 Host visualization

- Plate map with a ripple at the estimated (x, y), an uncertainty circle, and a live error heat map during evaluation.
- The **avatar turns its head and points toward the knock**: continuous control bridging Benchmark 1's discrete actions and Benchmark 2's continuous output.

---

## 7. Evaluation protocol

### 7.1 Dataset
- **Test grid:** 7 × 5 points, x ∈ {−12, −8, −4, 0, 4, 8, 12} cm, y ∈ {−8, −4, 0, 4, 8} cm (35 points, on the template). Where a point falls on a sensor module, knock 1 cm beside it and record the actual position.
- **15 knocks per point**, ≥ 2 people, medium fingertip/knuckle force → ≈ 525 knocks in collection mode.
- **Robustness sets:** soft vs. hard knocks; light pen tap vs. knuckle; an object resting on the plate; a person not in the training set; someone typing on the same table.
- All methods evaluated **offline on identical recordings**; the best ones then run live.

### 7.2 Splits
1. Random split by knock (positions seen in training).
2. **Leave-points-out** (positions never seen in training), the honest test of continuous localization.

### 7.3 Metrics
- **Benchmark 1:** 9-class accuracy and confusion matrix; errors also in cm (predicted zone → cell center).
- **Benchmark 2:** mean, median and 90th-percentile Euclidean error (cm); error heat map; derived zone accuracy for direct comparison.
- **System:** end-to-end latency knock → avatar reaction (phone slow-motion video, 240 fps ≈ 4 ms); false triggers per 10 min, idle and with table activity.

### 7.4 Success criteria
- **Benchmark 1:** ≥ 95 % zone accuracy for knocks near cell centers.
- **Quantization floor of Benchmark 1:** with 10 × 6.67 cm cells, even a perfect zone classifier that reports the cell center has a mean error of ≈ **3.2 cm** (mean distance from a uniform point in the cell to its center, computed numerically). **Benchmark 2 is meaningful only if it clearly beats this. Target: median error ≤ 2 cm.**
- **Latency:** ≤ 100 ms knock → on-screen reaction.

---

## 8. Test and verification plan

| ID | Test | Pass criterion |
|---|---|---|
| T1 | **Incoming module check:** each BMI160 alone on the breadboard (CS high, SAO low); read CHIP_ID; measure current; label 0–9 | All return 0xD1 |
| T2 | **Multiplexer + I8:** write `1 << k` to 0x70 and read back; I2C scan per channel | Channels 0–7 each show 0x68; 0x69 (I8) and 0x70 always visible |
| T3 | **Bus integrity:** all 9 sensors on 30 cm wires at 400 kHz; 10⁶ reads (~30 min); scope edges with the logic analyzer if available | 0 errors (fallback: 100 kHz, or remove duplicate pull-ups if the trunk is over-pulled) |
| T4 | **Throughput:** round-robin loop time (GPIO toggle or DWT counter); FIFO levels for 10 min | No FIFO overflow at 400 Hz |
| T5 | **Configuration and saturation:** samples per second per sensor; hardest knock right next to a module | ODR correct; peaks below ±16 g |
| T6 | **Noise floor:** each sensor stationary 60 s on the mounted plate; RMS | Thresholds ≥ 5σ |
| T7 | **Gain calibration:** 20 knocks 2 cm from each sensor toward the plate center → c_i; symmetry check with center knocks (I1 = I5, I3 = I7, four corners equal; the plate is rectangular, so I1 ≠ I3 is expected) | Spread after calibration < 10 % |
| T8 | **Mounting comparison** on an acrylic offcut (same 2 mm sheet if possible): cyanoacrylate vs. thin tape | Pick the higher, more repeatable amplitude |
| T9 | **Piezo front end:** VMID with the multimeter; idle ADC level; hardest knock with a scope/ADC log | VMID = 1.65 ± 0.05 V; idle ≈ mid-scale; clamps hold; pin stays within injected-current limits |
| T10 | **Piezo timing sanity:** knock on a piezo, then at the center | Nearest piezo fires first; center knock gives symmetric arrivals; v_eff estimated |
| T11 | **Ring-down:** record knock tails | Sets the refractory period |
| T12 | **Latency, false triggers and table-bump rejection** | ≤ 100 ms; ~0 false triggers / 10 min; table bumps rejected |
| T13 | **Soak and recovery:** 30 min continuous; unplug/replug one sensor cable | No hang; I2C bus recovery (9 SCL pulses + re-init) restores operation |
| T14 | **Power:** total 3.3 V current, multimeter in series | Well within the board's regulator budget |

---

## 9. Milestones and roles

| Milestone | Content | Owner role |
|---|---|---|
| M1 | Module prep (straps, headers); breadboard bring-up T1–T6, T9 | Electronics |
| M2 | Mounting test T8; plate assembly; T7, T10, T11 | Electronics + Assembly |
| M3 | Firmware acquisition pipeline and collection mode; record the dataset | Firmware |
| M4 | Benchmark 1 live demo with avatar | Firmware + Host software |
| M5 | Benchmark 2 methods offline; best method live | Host software / analysis |
| M6 | Evaluation (§7), report, demo rehearsal (T12, T13) | All |

---

## 10. Risks and mitigations

| Risk | Mitigation |
|---|---|
| I2C bandwidth caps the accelerometers at 400 Hz | Piezos supply high-frequency information; if needed, move some sensors to a second I2C peripheral on the Arduino header |
| Thin plate: added mass (modules, glue, cables) perturbs modes | Electronics off-plate; flush-trimmed modules; one cable exit edge; per-channel calibration; modes handled by regression (2d) |
| Neighbor sensors close together → zone confusion | Crosstalk guard; gain calibration; report Benchmark 1 confusion matrix honestly |
| Fast edge reflections on a small plate corrupt TDOA | Short onset windows, band-pass filtering, first-arrival detection; regression as fallback |
| 2 mm acrylic cracks | No drilling; finger/knuckle knocks; spare sheet if available |
| Module boots in SPI mode / address clash | CS and SAO strapped on every module; verified in T1/T2 |
| Table vibration → false triggers | Foam feet; table-reference LSM6DSL rejection |
| Campus Wi-Fi isolates clients | USB virtual COM port by default; phone hotspot for the Wi-Fi demo |
