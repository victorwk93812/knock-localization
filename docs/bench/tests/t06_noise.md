# t06_noise — T6: noise floor

**Proves:** how noisy each sensor is at rest. Five times the noise of the typical
sensor (the median) is the lowest usable knock threshold. A sensor much noisier
than the rest has a mounting or cable problem.

## You need

The full array. Run it **twice**: once on the breadboard (a reference for each
module by itself) and once on the assembled plate standing on its feet, which
gives the numbers the firmware will use. A quiet minute: no typing, no walking,
no fans on the table.

## Wiring

Same as [t02](t02_mux_scan.md). The onboard LSM6DSL is recorded too, as the
table reference; the Discovery board must sit on the same table as the plate.

## Steps

1. Terminal 1, from `host/` — **first**:

   ```sh
   uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench
   ```
2. Terminal 2, from `firmware/`:

   ```sh
   cargo run --release --bin t06_noise
   ```

   (`--features knock-bench/quick` gives a 10 s run for a smoke test.)
3. **Hands off the table for 60 s.**
4. When `RESULT` appears, the script prints its table and the file name. Stop the
   script with Ctrl-C.

## What you get

In the log, per sensor: the number of samples and, in mg, mean and RMS noise per
axis, mean |a| (about 1000 at rest), the RMS of the dynamic magnitude, and five
times that. Below the table:

```text
INFO: BMI160 dynamic RMS: median 2.45 mg over 9 sensors; noisiest I8 at 2.80 mg
INFO: suggested trigger threshold: 12.2 mg (5 x the median dynamic RMS)
INFO: for comparison, 5 x the noisiest sensor (I8) would be 14.0 mg
```

From the script (example):

```text
t06_noise: noise summary [mg] (5sigma = 5 x dynamic RMS)
sensor  mean_x  mean_y  mean_z  rms_x  rms_y  rms_z     |a|  dyn_rms  5sigma
    I0   -12.4     8.0  1000.0   1.20   1.20   1.20  1000.1     2.00   10.00
   ...
 table    -3.4   -10.0  1027.0   0.54   0.54   0.54  1027.0     0.90    4.50
BMI160 (I0..I8): median dynamic RMS 2.45 mg over 9 sensor(s); noisiest I8 at 2.80 mg
suggested trigger threshold: 12.2 mg (5 x the median dynamic RMS)
for comparison, 5 x the noisiest BMI160 (I8) would be 14.0 mg
```

The threshold is 5 × the **median** dynamic RMS of the BMI160s that delivered
samples: the typical sensor. The "for comparison" line shows what the noisiest
sensor alone would need. The two are close on a healthy plate.

plus a `WARNING` for any sensor above 3 × the median and `MISSING` for any sensor
without data. "table" is the onboard reference sensor; it is a different chip, so
its noise is not comparable with the BMI160s.

## Pass

- All nine BMI160s and the table reference delivered samples (`RESULT: PASS`).
- No sensor above 3 × the median.
- The chosen trigger threshold is at least 5 × the median noise.

## Write down

Median dynamic RMS; the noisiest sensor and its value; the suggested threshold
and the comparison value for the noisiest sensor. Keep the CSV file name.

## If it fails or looks odd

| Symptom | Likely cause |
|---|---|
| The script saves nothing | It was started after the test. t06 sends its header once, at the end of the 60 s; start the script first |
| One sensor several times noisier | Loose glue joint, a cable pulling on or touching the plate, or a bad module. Compare with its breadboard run |
| All sensors noisy, table reference too | The table is vibrating: fan, people, building. Repeat at a quiet moment |
| All sensors noisy, table reference quiet | The plate is being excited: cables, air flow, a foot not touching |
| |a| far from 1000 mg on one sensor | Offset of that module. Not a problem for knock detection, which uses the dynamic part |

The suggested threshold follows the **median**, so one bad module does not
inflate it. That module can then trigger by itself at this threshold: if the
"for comparison" value is much larger than the suggestion, fix the outlier
first, then take the number.
