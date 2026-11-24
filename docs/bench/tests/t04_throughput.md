# t04_throughput — T4 and T14: throughput and supply current

**Proves:** the firmware can empty nine sensor buffers every 20 ms for ten minutes
without losing a sample (T4). The same run is the steady load for measuring the
total 3.3 V current (T14).

## You need

The full array as in t02/t03; t03 passing at 400 kHz. Multimeter. Optional: logic
analyzer.

## Wiring

Same as t02, plus:

- **For T14:** the meter in series in the supply. Unplug USB. Remove the wire
  from the board's 3V3 pin to the breadboard + rail and put the meter there: red
  probe to the board's 3V3, black probe to the + rail, red lead in the **mA**
  jack. Plug USB back in.
- **Optional:** logic analyzer channel on Arduino **D2**, ground on GND. D2 is
  high while a readout round runs and low while the loop waits: pulse width =
  loop time, period = 20 ms.

## Steps

1. Smoke run (10 s): add `--features knock-bench/quick`.
2. Full run, 10 minutes:

   ```sh
   cargo run --release --bin t04_throughput
   ```

   Leave the sensors alone. A summary line appears every 60 s.
3. **T14:** read the meter while it runs and write the value down.
4. Afterwards: unplug USB, take the meter out, put the supply wire back, and
   **move the meter's red lead back to the V/Ω jack**.

## What the log shows

Every 60 s and in a final table per sensor:

- **loop time** min / avg / max in µs: how long one round over all nine sensors
  takes;
- **highest buffer level** per sensor in frames: about 8 is normal (400 Hz ×
  20 ms), 170 is full;
- **overflows**: must stay 0;
- **frames received** against 400 Hz × elapsed time;
- **I2C errors** per sensor.

## Pass

- All nine sensors configured.
- 0 overflows, 0 I2C errors.
- Maximum loop time below 20 000 µs.
- T14: the current is a few tens of mA at most (each BMI160 draws well under
  1 mA; power LEDs on the modules dominate). Far more means a short or a wrong
  part.

## Write down

Overflows, maximum loop time in ms, highest buffer level, supply current in mA.

## If it fails

| Symptom | Likely cause |
|---|---|
| A sensor "not configured" at the start | Re-run t02 |
| Maximum loop time above 20 ms now and then | Bus retries: go back to t03. If built with `i2c-100k`, this is expected |
| Overflows on one sensor | That sensor's readout fails intermittently: its wires |
| Frame count a few % off on every sensor | Compare with t05; the board's clock and the sensors' clocks differ by up to about 2 % |
| Meter reads 0 mA and nothing works | Blown meter fuse, or the lead is in the wrong jack |
