# t03_bus_soak — T3: bus integrity

**Proves:** the I2C bus carries one million transfers to nine sensors over the
final 30 cm wires without a single error. Random bus errors later look like
missed knocks, so this has to be clean.

## You need

The full array as in [t02](t02_mux_scan.md), **with the wires that will be used
on the plate** (30 cm female–female to each module). t02 passing. About 30 minutes.
Optional: logic analyzer.

## Wiring

Same as t02. Lay the cables the way they will run later: side by side as a flat
bundle, not coiled.

## Steps

1. Smoke run, about 10 s (20 000 transfers):

   ```sh
   cargo run --release --bin t03_bus_soak --features knock-bench/quick
   ```
2. Full run:

   ```sh
   cargo run --release --bin t03_bus_soak
   ```

   **Do not touch the wiring while it runs.** A progress line appears every
   50 000 transfers.
3. Optional, with the logic analyzer: look at SCL and SDA on the trunk and on one
   channel. Rising edges should reach the top well within the high half of the
   clock; low levels should sit near 0 V.

## What the log shows

A round visits the nine sensors once: select the channel, read CHIP_ID, read six
data bytes (26 transfers per round). Each transfer is counted against its sensor
as OK, NACK, timeout, wrong CHIP_ID or other. At the end there is one row per
sensor with these counters, and `RESULT`.

If the bus hangs, the firmware recovers it and continues; more than 50 recoveries
abandons the test.

## Pass

1 000 000 transfers, **0 errors on every sensor**.

## Write down

Total errors (must be 0); which fallback was needed, if any.

## If it fails

| Pattern | Meaning | Fix |
|---|---|---|
| Errors on one sensor only | Its wires or sockets | Replace those four wires; re-run |
| Errors on I8 and on all channels | The trunk | Check the trunk rows, the wires to D14/D15, the supply |
| Errors everywhere, rising edges slow and rounded | Pull-ups too weak for the cable capacitance | Add 4.7 kΩ from SDA to 3V3 and from SCL to 3V3 on the trunk |
| Low level doesn't come near 0 V | Too many pull-ups in parallel on the trunk | Remove the pull-ups on module 8 or on the multiplexer board |
| Errors only when cables are moved | A loose socket | Find it by wiggling one cable at a time during a quick run |

**Fallback, 100 kHz:**

```sh
cargo run --release --bin t03_bus_soak --features knock-bench/i2c-100k
```

If only 100 kHz is clean, tell the team: at 100 kHz the bus is too slow to read
nine sensors every 20 ms, so t04 will fail and the design needs a change (shorter
wires, stronger pull-ups, or fewer bytes per round).
