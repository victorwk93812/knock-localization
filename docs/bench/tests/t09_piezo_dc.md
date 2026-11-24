# t09_piezo_dc — T9: piezo front end

**Proves:** each piezo input idles at mid-scale with low noise, each channel
answers its own disc, the sample rate is right, and a hard knock is limited by
the clamps without upsetting the board.

Read the t09 notes in [../STATUS.md](../STATUS.md) first: they explain why a hard
knock is *expected* to hit the ADC limits.

## You need

- 4 piezo discs with leads ([../materials.md](../materials.md) §5). They can lie
  loose on the table or be taped to the offcut; they need not be glued yet.
- Breadboard parts: 2 × 100 kΩ, 1 × 10 µF, 1 × 100 nF, and per channel 1 × 1 MΩ,
  1 × 10 kΩ, 2 × 1N4148, 1 × 1 nF (so 4 of each, 8 diodes).
- 4 wires to A0–A3, multimeter.

## The circuit

Shared bias, built once:

```
  3V3 ──[100 kΩ]──┬──[100 kΩ]── GND
                  │
                 VMID ≈ 1.65 V
                  │
                  ├──┤├── GND     10 µF, + side to VMID, stripe (−) to GND
                  └──┤├── GND     100 nF
```

One channel (build four, j = 0…3):

```
                                        3V3
                                         │
                                        ─┴─  1N4148, band toward 3V3
                                         ▲
  piezo red ───── Pj ──[10 kΩ]────┬──────┼──────┬────► Aj on the board
  (ceramic)        │              │      │      │
                [1 MΩ]            │     ─┴─    ═╧═ 1 nF
                   │              │      ▲      │
  piezo black ─── VMID          ADCj     │     GND
  (brass)                               GND
                                    1N4148, band toward ADCj
```

In words, per channel:

- piezo **black** (brass) to VMID; piezo **red** (ceramic) to node Pj;
- 1 MΩ from Pj to VMID;
- 10 kΩ from Pj to node ADCj;
- upper diode from ADCj to 3V3 with its **band at 3V3**;
- lower diode from GND to ADCj with its **band at ADCj**;
- 1 nF from ADCj to GND;
- a wire from ADCj to A*j*.

Channel j is piezo Pj: A0 = P0, A1 = P1, A2 = P2, A3 = P3.

**Breadboard layout that avoids mistakes:** give VMID its own rail (the second
power rail on one side). Build each channel in its own block of three rows (Pj,
ADCj, and nothing else), leaving an empty row between channels. Build channel 0
completely, check it, then copy it three times.

## Before connecting A0–A3

Do these with the four wires to A0–A3 **not yet plugged in**.

1. **Pin voltage limit (once, on paper).** The clamps let ADCj reach about
   3.95 V. Confirm in the STM32L475 datasheet, absolute maximum ratings, that
   this input voltage and the resulting injected current are allowed on these
   pins. If not, or if in doubt: use **22 kΩ** in place of 10 kΩ and **470 pF**
   in place of 1 nF on every channel. Write down which values you built.
2. **Power on, DC volts:**
   - VMID = 1.65 ± 0.05 V.
   - Each ADCj node reads about **1.5 V**, not 1.65 V. That is the meter's own
     10 MΩ loading the 1 MΩ path, not a fault. A reading near 0 V or 3.3 V is a
     fault.
3. **Power off, diode mode, per channel:** red probe on ADCj, black on 3V3 →
   about 0.6 V; swapped → open. Red on GND, black on ADCj → about 0.6 V; swapped
   → open. (In circuit the "open" direction may show a slowly rising value as the
   capacitors charge; it must not show 0.6 V.)
4. Plug in the wires to A0–A3. Run [t00_board](t00_board.md): each `piezo A*j*`
   line should now read about 2048 counts / 1650 mV.

## Steps

1. Optional, to keep the waveforms: terminal 1, from `host/`, **first**:

   ```sh
   uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench
   ```
2. Terminal 2, from `firmware/`:

   ```sh
   cargo run --release --bin t09_piezo_dc
   ```
3. **Phase 1, 5 s: touch nothing.** Idle level, noise and sample rate are
   measured.
4. **Phase 2 starts when LED2 turns on** (LED2 on = armed). Tap disc 0, wait for
   LED2 to come back on (each knock takes about 0.4 s to send, and knocks during
   that time are ignored), then disc 1, 2, 3.
5. Give the **hardest knock you intend to use** next to one disc.
6. Hold the blue button for about half a second to finish.

## What the log shows

Phase 1, per channel:

```text
PASS: sample rate 100000 S/s per channel (expected 100000 within 1 %)
INFO: A0: mean 2046.3 counts (1649 mV), RMS 1.42, min 2042, max 2050 over 500000 samples
PASS: A0: idle mean 2046.3 counts (expected 2048 +- 150 ...)
PASS: A0: idle RMS 1.42 counts (limit 8 ...)
```

Phase 2, per knock and channel:

```text
--- knock 3 (trigger on A1) ---
INFO: knock 3 A1: min 0 max 4095 peak dev 2049 counts (1651 mV), FLAT-TOP high, AT ADC RAIL
```

`FLAT-TOP` = at least 5 samples in a row at the extreme: the signal was limited.
`AT ADC RAIL` = a sample was 0 or 4095: the pin went outside 0–3.3 V and the
diodes carried current. Both are warnings.

The script prints a table of baseline and peak per knock (`*` marks a rail hit)
and saves plots of the waveforms.

## Pass

- Sample rate within 1 % of 100 000 per channel.
- Idle mean within 2048 ± 150 counts and idle RMS below 8 counts on all four.
- Every channel responded to at least one knock.
- No samples lost while armed.
- After the hard knock the firmware is still running and the idle level returns
  (run t00 or look at the next knock's baseline).

## Write down

Idle counts and RMS per channel; the peak of the hardest knock; whether it
flat-topped or hit a rail; the resistor and capacitor values built.

## If it fails

| Symptom | Likely cause |
|---|---|
| Sample rate wrong | The ADC clock setting in the firmware is wrong (it is unverified). Report the number; do not continue to t10 |
| One channel idles near 0 or 4095 | A diode is in backwards, or the 1 MΩ is missing or in the wrong row |
| All channels idle at the wrong level | VMID is wrong: check the two 100 kΩ and the 10 µF polarity |
| Idle RMS high on one channel | Floating input (wire to A*j* missing), missing 1 nF, long untwisted lead |
| Idle RMS high on all | Hum on VMID: missing 10 µF / 100 nF |
| A channel never responds | Disc open or cracked (re-check with the meter), leads swapped to the wrong rows |
| Wrong channel responds | Wires to A0–A3 crossed |
| `inputs never settled` | An input keeps moving: floating, or someone is touching the table |
| Knocks trigger constantly | Trigger (60 counts) too low for this set-up. Report the idle RMS |
| Board resets on a hard knock | Stop. The clamp is not doing its job; re-check the diodes and consider the 22 kΩ option |
