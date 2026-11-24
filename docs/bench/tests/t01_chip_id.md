# t01_chip_id — T1: incoming check of each BMI160 module

**Proves:** each module answers on I2C at the address its SAO strap selects, is a
real BMI160 (`CHIP_ID = 0xD1`), accepts the project configuration, and measures
gravity as 1 g.

## You need

- All 10 modules, soldered, strapped and labeled
  ([../materials.md](../materials.md) §4).
- 4 jumper wires. The board's header sockets take **male** ends and a module's
  header pins take **female** ends, so use male–female wires, or go through the
  breadboard.
- Multimeter (for the current reading).

## Wiring

One module at a time, straight to the board. **No multiplexer.**

```
  Discovery board                 GY-BMI160 module
  3V3  ───────────────────────── VIN   (or 3V3, see materials §4.4)
  GND  ───────────────────────── GND
  D15  ───────────────────────── SCL
  D14  ───────────────────────── SDA
                                 CS  ──┐ strapped on the module
                                 SAO ──┘ (CS → 3V3; SAO → GND or 3V3)
```

Find D14 and D15 by the labels printed beside the digital header (they are at
its end, next to AREF and GND).

## Steps

1. Wire module 0. Lay it flat, component side up, and don't touch it.
2. From `firmware/`:

   ```sh
   cargo run --release --bin t01_chip_id
   ```
3. Read the lines for `module 0`. While the firmware waits, LED2 is **steady on**
   if that module passed and **blinks fast** if it failed.
4. Unplug the module's wires at the module, plug in the next module, press the
   **blue button**. The firmware counts 0, 1, 2 … 9, so test the modules in label
   order. (Swapping modules with power on is acceptable here; connect GND first.)
5. After module 9 the test ends by itself. To stop early, hold the blue button for
   2 s.
6. **Current, once per module:** put the meter (mA range) in series in the 3V3
   wire while the module is being sampled. Expect about 1 mA or less, more if the
   module has a power LED. A module far from the others is suspect.

## What the log shows

```text
--- module 0 ---
PASS: module 0: found at 0x68 (SAO strap low: fits I0..I7 behind the mux)
PASS: module 0: CHIP_ID = 0xd1 (expected 0xd1)
PASS: module 0: 400 samples in 1000 ms (expected 360..440 at 400 Hz)
INFO: module 0: mean x -12, y 8, z 1003 mg
INFO: module 0: RMS noise x 1.2, y 1.3, z 1.9 mg
PASS: module 0: |a| = 1003 mg (expected 900..1100 at rest)
module 0: OK
```

(Numbers are examples.) `0x68` means SAO is low, `0x69` means SAO is high.

## Pass (per module)

- Address `0x68` for modules 0–7 and 9; `0x69` for module 8.
- `CHIP_ID = 0xd1`.
- 360–440 samples in one second.
- |a| between 900 and 1100 mg. Lying flat, z is about ±1000 mg and x, y are near 0.

At the end: `INFO: 10 of 10 checked modules passed`.

## Write down

Per module, in the checklist table: address, CHIP_ID, |a|, current.

## If it fails

| Symptom | Likely cause |
|---|---|
| Nothing found | CS not strapped to 3V3 (chip is in SPI mode: fix the strap and **power-cycle** the module); no power; SDA and SCL swapped; bad wire |
| Both 0x68 and 0x69 found | Two modules connected, or the multiplexer is still on the bus |
| Wrong address | SAO strapped the other way, or left floating |
| `wrong CHIP_ID` | Not a BMI160. Set the module aside |
| Too few / too many samples | Report the count; the sample-rate setting may be wrong |
| |a| far from 1000 mg | Module was moving; otherwise a damaged sensor |
| `bus recovery` lines | A wire came loose mid-transfer. Reseat and repeat |
