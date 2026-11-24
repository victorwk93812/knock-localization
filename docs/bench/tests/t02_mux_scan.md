# t02_mux_scan — T2: multiplexer and I8

**Proves:** the multiplexer answers, each of its eight channels reaches exactly one
sensor, and I8 is visible beside it on the trunk.

## You need

- TCA9548A module with its header soldered (pins down, so it plugs into the
  breadboard).
- Modules 0–8, all passed in t01.
- Breadboard, short jumpers, 9 × 4 wires for the modules. The modules may lie
  loose on the table for this test.

## Before powering: multiplexer checks with the meter

With the multiplexer in the breadboard and powered once, DC volts:

| Pin | Must read | Why |
|---|---|---|
| A0, A1, A2 | about 0 V | Address 0x70. If they float (no resistors on your board), wire all three to GND |
| RST (RESET) | about 3.3 V | Low holds the chip in reset. If it floats, wire it to 3V3 |

## Wiring

```
  Discovery            breadboard
  3V3 ───────────────► + rail ──┬── TCA9548A VIN        ┌── module k VIN   (k = 0…8)
  GND ───────────────► − rail ──┼── TCA9548A GND        ├── module k GND
                                │
  D14 (SDA) ───► trunk SDA row ─┼── TCA9548A SDA ───────┼── module 8 SDA   (I8, 0x69)
  D15 (SCL) ───► trunk SCL row ─┴── TCA9548A SCL ───────┴── module 8 SCL

  TCA9548A SD0 / SC0 ──► module 0 SDA / SCL
  TCA9548A SD1 / SC1 ──► module 1 SDA / SCL
     …
  TCA9548A SD7 / SC7 ──► module 7 SDA / SCL
```

- "Trunk row" = one breadboard row for SDA and one for SCL. Each holds three
  things: the wire from the board, the multiplexer's upstream pin, module 8.
- Modules 0–7 connect **only** to their channel pins, never to the trunk rows.
- Every module gets power and ground from the rails.
- Use a fixed wire color per signal. Most faults in this test are a wire one hole
  off.

## Steps

1. **First with one module.** Wire the multiplexer, module 8 on the trunk, and
   one known-good module on channel 0. Run:

   ```sh
   cargo run --release --bin t02_mux_scan
   ```

   Channel 0 must be `ok`, channels 1–7 `FAIL` (nothing there yet). Move the
   module to channel 1, 2 … 7 and re-run each time if you want to prove every
   channel separately from every module.
2. **Then all nine.** Wire modules 0–7 to channels 0–7 and run it again.

## What the log shows

```text
=== T02 mux scan ===
INFO: I2C1 at 400000 Hz
PASS: mux answers at 0x70
ch | readback | 0x68 | 0x69 | 0x70 | CHIP_ID | result
 0 |   0x01   |  yes |  yes |  yes |    0xd1 | ok
 1 |   0x02   |  yes |  yes |  yes |    0xd1 | ok
 …
PASS: all channels off: 0x68 absent
PASS: I8: BMI160 at 0x69 on the trunk, CHIP_ID = 0xd1 (expected 0xd1)
INFO: verdict: all 8 mux channels OK
INFO: verdict: I8 (0x69) OK
RESULT: PASS
```

`readback` is the channel mask the multiplexer reports: `1 << k`, so 0x01, 0x02,
0x04 … 0x80.

## Pass

- Multiplexer at `0x70`.
- Every channel: readback correct, `0x68` present with `CHIP_ID 0xd1`, and `0x69`
  and `0x70` visible.
- With all channels off, `0x68` is absent.
- I8 at `0x69` with `CHIP_ID 0xd1`.

## Write down

Failing channels, if any.

## If it fails

| Symptom | Likely cause |
|---|---|
| `no multiplexer` | Its power; SDA/SCL swapped; A0–A2 not low; RST low or floating |
| One channel: nothing at 0x68 | That channel's two wires, that module's power, or its straps |
| One channel shows the wrong readback | Multiplexer fault; try the spare |
| `0x68` present with all channels off | A 0x68 module is on the trunk rows by mistake |
| `unexpected devices` | Something else on the bus; a module answering at the wrong address |
| I8 missing | Module 8 not on the trunk, or its SAO strap is not high |
| Everything fails after adding the ninth module | Supply sagging: measure the rail; or one module shorting the bus: add modules one at a time |

Re-run this test **after gluing** the modules to the plate: it is the quickest
check that every cable survived.
