# t07_capture — T7 and T8: knock capture

One binary, two uses:

- **T8, before assembly:** which mounting (cyanoacrylate or thin tape) couples a
  module better to the acrylic.
- **T7, on the assembled plate:** a gain factor c_i for each sensor, so that equal
  knocks give equal readings, and a symmetry check.

**What it does:** watches all sensors, and when any of them sees more than 300 mg
of dynamic acceleration it saves 20 samples (−10 ms … +40 ms) from every sensor,
logs each sensor's peak, and sends the samples as CSV. LED2 toggles at every
captured knock. The blue button ends the run.

## Rules for every run

1. **Start the capture script first, then the test.** Then every knock is in the
   file. The data header is repeated before every knock, so a script started
   later still records the knocks that follow, but not the earlier ones.
2. **End every run with the blue button.** Only then is the file complete.
3. **One run = one file.** For the next run, start the script again, then
   `cargo run` again.
4. Leave at least half a second between knocks. Knock with a fingertip or
   knuckle, the same way each time: the calibration is only as good as the
   repeatability of your knocks. The script prints that repeatability as CV %.
5. Sensors that are not connected are skipped, not treated as an error.

---

## T8 — mounting comparison (two modules on an offcut)

### You need

- An acrylic offcut, ideally from the same 2 mm sheet, at least about 10 × 10 cm,
  on foam feet or a folded cloth.
- **Module 8** (address 0x69) and **module 9, the spare** (0x68). Without a
  multiplexer the firmware calls the 0x68 module I0 and the 0x69 module I8.
- Cyanoacrylate, thin double-sided tape, a marker.

### Wiring

Both modules directly on the trunk, no multiplexer:

```
  3V3 ─┬─ module 9 VIN      GND ─┬─ module 9 GND
       └─ module 8 VIN           └─ module 8 GND
  D14 ─┬─ module 9 SDA      D15 ─┬─ module 9 SCL
       └─ module 8 SDA           └─ module 8 SCL
```

### Steps

Two modules never have exactly the same sensitivity, so compare each mounting
against the same reference:

1. Mark a knock point. Mark two module positions at the **same distance** from it
   (for example 4 cm left and 4 cm right).
2. **Run A, both on tape.** Tape both modules down. Capture 20 knocks at the mark:

   ```sh
   # terminal 1, host/
   uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench
   # terminal 2, firmware/
   cargo run --release --bin t07_capture
   ```

   Knock 20 times, press the blue button. Note the mean peak of I0 and of I8 and
   their ratio I0 / I8.
3. **Run B, spare on glue.** Lift module 9, clean off the tape, glue it at the
   same spot with a thin layer of cyanoacrylate, wait until it has set. Module 8
   stays on its tape. Capture 20 knocks again.
4. Compare the ratio I0 / I8 between run A and run B. If it went **up**, glue
   couples better than tape by that factor. Also compare the knock-to-knock
   scatter of I0 in both runs.

Module 9 stays on the offcut afterwards; that is what the spare is for. If you
would rather keep it, skip run A and accept that the comparison then includes the
difference between the two modules.

### Pass

A decision: the mounting with the higher and more repeatable peak. Write the two
ratios and the choice in the checklist.

---

## T7 — gain calibration (assembled plate)

### You need

The assembled plate on its feet, t02 and t06 passing on it, the paper template
underneath showing the sensor positions. About 30 minutes.

### Wiring

The full array as in [t02](t02_mux_scan.md).

### Steps

1. **Nine runs, one per sensor.** For sensor K = 0 … 8:

   ```sh
   # terminal 1, host/
   uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench --mode gain --sensor K
   # terminal 2, firmware/
   cargo run --release --bin t07_capture
   ```

   Knock **20 times, 2 cm from sensor K, on the side toward the plate center**.
   For I8 (the center sensor) pick one side and note it. Press the blue button.
   The script prints the mean, the standard deviation and the CV of sensor K's
   peak. It warns if another sensor was louder than K: then the knock position,
   `--sensor`, or two cables are wrong. It also notes if the file doesn't hold
   exactly 20 knocks. Stop the script and rename or note the file as "sensor K".
2. **One center run.** Same commands without `--mode gain --sensor K`. Knock 20
   times at the plate center. I8 sits there, so knock on the plate directly
   beside it, as close to the center mark as you can, and always on the same
   spot. Press the blue button. Being 1–2 cm off center tilts the symmetry
   numbers a little; if a pair fails narrowly, repeat with knocks on the
   opposite side of I8 and average the two.
3. **Solve.** With the nine files in sensor order and the center file:

   ```sh
   uv run bench/capture.py --mode gain-solve \
       ../data/bench/<sensor0>.csv ../data/bench/<sensor1>.csv … ../data/bench/<sensor8>.csv \
       --center ../data/bench/<center>.csv
   ```

### What you get

```text
gain-solve: file k = knocks near sensor k
sensor  knocks  R_i [mg]  CV [%]    c_i  note
    I0      20    1850.2     6.1  1.021
    …
centre knocks (20) after calibration:
  …
  T7 (spread < 10 %): PASS
```

`R_i` is sensor i's mean response to its own knocks, `c_i` the gain that
equalises them.

### Pass

After calibration, for center knocks: I1 and I5 agree within 10 %, I3 and I7
agree within 10 %, and the four corners (I0, I2, I4, I6) agree within 10 %.
I1 ≠ I3 is expected: the plate is rectangular.

### Write down

The nine gains c_i, the three symmetry numbers, the largest CV, the file names.

---

## If it fails

| Symptom | Likely cause |
|---|---|
| The first knocks are missing from the file | The script was started after the test. It records from the first knock that begins after it attached. Start it first |
| Script saves nothing | No knock was captured after the script attached (see the next row), or the wrong serial port |
| A peak shows as `-` (`nan` in the file) | That sensor was absent for that knock: no data from it. It is left out of the statistics, not counted as 0 |
| No knock is captured | Knocks below 300 mg at every sensor. Knock closer or harder; check t06 for dead sensors |
| Knocks captured without knocking | Trigger too low for this set-up, or a cable slapping the plate. Compare with the t06 threshold |
| One sensor always absent | Its cable. Run t02 |
| CV above about 15 % | Knocks too uneven. Use the same finger, same height, same spot; repeat the run |
| "loudest is I*j*" for the run of sensor K | Channels j and K are swapped at the multiplexer, or the module sits in the wrong cell |
| Symmetry fails with good CV | A mounting difference (glue layer, air gap) or a cable loading one side. Compare with t06 noise for the same sensor |
| `incomplete knock(s) left out` | The run was cut off before the blue button |

More detail on the data format and its limits: [../STATUS.md](../STATUS.md).
