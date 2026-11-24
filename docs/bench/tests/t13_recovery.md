# t13_recovery — T13: soak and recovery

**Proves:** over 30 minutes of normal acquisition, the firmware survives sensors
being unplugged and plugged back: it notices, carries on with the others,
brings the sensor back, and never hangs.

## You need

The complete assembled set-up; t04 passing. 30 minutes, and someone to pull
wires.

## Wiring

The full array as in [t02](t02_mux_scan.md). Decide beforehand which wires you
will pull, and pull them **at the breadboard end**, so nothing tugs on the plate.

## Steps

1. Smoke run (10 s, no unplugging): add `--features knock-bench/quick`.
2. Full run:

   ```sh
   cargo run --release --bin t13_recovery
   ```
3. Let it run undisturbed for a few minutes. A summary appears regularly.
4. **Pull one sensor's SDA wire.** Wait about 5 s. Plug it back. Watch the log:
   the sensor is reported as stopped, then as back, with the outage time.
5. Repeat with: another sensor's SCL wire; a sensor's power wire; I8 (on the
   trunk); and, if you like, the multiplexer's own SDA.
6. Leave a few minutes between pulls so the events are easy to tell apart in the
   log.
7. **Have every wire plugged back in at least a minute before the 30 minutes are
   up.** A sensor still missing at the end fails the test.

## What the firmware does

- A sensor whose readout fails is logged, taken out of the round, and probed
  every 500 ms. The others keep being read every 20 ms.
- When it answers again it is reset and reconfigured, and the outage duration is
  logged.
- If a transfer hangs (bus stuck), the firmware clocks the bus free (9 clock
  pulses and a STOP) and carries on. This is counted as a bus recovery.
- A watchdog resets the board if the firmware itself stops for 2 s. The test
  then starts over, says `reset by watchdog`, and fails.

## Pass

- No watchdog reset.
- Every sensor that came back was reconfigured successfully.
- All sensors healthy at the end.

The final report lists outages and recoveries per sensor, the longest outage, bus
recoveries, and the longest gap between two good readouts of a healthy sensor
(20 ms nominal).

## Write down

Time to recover after an SDA pull and after a power pull; number of bus
recoveries; the longest gap.

## If it fails

| Symptom | Likely cause |
|---|---|
| Outages that nobody caused | A loose socket. This is the fault the test exists to find: replace that wire |
| Every sensor reported down after one pull, and they stay down | The pulled sensor's line is holding the bus low and the multiplexer cannot be told to drop it (its RESET pin is not wired). Unplug that sensor fully, or power-cycle. Known limit, see [../STATUS.md](../STATUS.md) |
| A sensor comes back but re-init fails | Plugged back with power but without a data line; reseat all four wires |
| `reset by watchdog` | A firmware hang. Save the whole log and report it |
| Board resets when a power wire is plugged back | Inrush dipping the 3.3 V rail. Note it; plug power back more gently or add 10 µF on the rail |
| Fails when built with `i2c-100k` | Expected: the bus is too slow at 100 kHz |

Do not halt the program with a debugger during this test: the watchdog keeps
counting and resets the board after 2 s.
