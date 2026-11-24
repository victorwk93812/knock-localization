# t00_board — board self-check

**Proves:** the Discovery board, the probe, the serial port and your tool
installation work, before anything is attached.

## You need

The Discovery board and a data-capable micro-USB cable, plugged into the
**ST-LINK** USB socket (the one that makes `probe-rs list` show a probe). Nothing
soldered, nothing wired. Lay the board flat and still on the table.

## Steps

1. Terminal 1, from `firmware/`:

   ```sh
   cargo run --release --bin t00_board
   ```
2. Watch LED2 (green): it blinks three times at the start.
3. Read the log (below).
4. Terminal 2, from `host/`, the serial echo check:

   ```sh
   uv run bench/capture.py --port /dev/ttyACM0 --echo
   ```

   Type a line and press Enter, a few times. Each line must come back unchanged
   (`OK: sent ..., received ...`; `MISMATCH` is a failure). Ctrl-D quits (Windows: Ctrl-Z, then Enter). On Windows or macOS use your own port name
   in place of `/dev/ttyACM0` ([../README.md](../README.md) §2). The reset banner is only shown if
   the script was already listening when the board restarted; that is optional.

## What the log shows

- `=== T00 ... ===`, the clock and bus settings.
- `PASS` for the onboard LSM6DSL answering `WHO_AM_I = 0x6A`.
- `PASS` for gravity: |a| between 800 and 1200 mg with the board at rest.
- `INFO: I2C1 scan: nothing answered` — expected, nothing is wired to D14/D15.
  If sensors are already wired, their addresses are listed here.
- `INFO: piezo A0…A3: <counts> = <mV>` — one reading per analog input. With
  nothing connected the pins float and **any value is fine**. Once the front end
  is wired, each should read about 2048 counts / 1650 mV, which makes t00 a quick
  check later.
- `RESULT: PASS`, then LED2 blinks slowly.

## Pass

- LED2 blinked; `RESULT: PASS`.
- Echo is clean at 921 600 baud.

## Write down

|a| in g; whether the echo was clean (if not, the highest baud rate that is).

## If it fails

| Symptom | Likely cause |
|---|---|
| `probe-rs` finds no probe | Charge-only cable; wrong USB socket; udev rules missing (Linux); ST driver missing (Windows); another program holds the probe |
| Flashing works, no log lines | Old probe-rs; update to 0.32 |
| LSM6DSL does not answer | Board fault, or a firmware bug: report the log |
| |a| far from 1 g | Board was moving; otherwise the sensitivity constant is wrong (it is unverified, see [../STATUS.md](../STATUS.md)): report the value |
| `Permission denied` on the port | Linux serial group, [../README.md](../README.md) §2.3 |
| Echo garbled or bytes missing | Note it. Try `--baud 115200` only to confirm the port works; the firmware itself still talks at 921 600, so a mismatch gives garbage. Report it and the baud rate gets lowered in the firmware |
