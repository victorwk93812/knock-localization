# Bench tests

How to check every part and run every test of `Final_Proposal_v2_EN.md` §8.
This page gets a new tester from an empty laptop to the first passing test.

| Page | What it covers |
|---|---|
| this page | The hardware, installing the tools, how a test is run, the order of tests |
| [materials.md](materials.md) | Checks with the multimeter only; soldering and preparing the modules and piezo discs |
| [tests/](tests/) | One page per test binary: wiring, steps, expected output, pass criteria, faults |
| [CHECKLIST.md](CHECKLIST.md) | Tick boxes and blanks for measured values, by milestone |
| [STATUS.md](STATUS.md) | What has not been proven yet: first-guess thresholds, unverified values, known limits |

> **Nothing here has run on the board yet.** The test binaries compile and their
> logic is unit-tested on a PC, nothing more. Expect to find firmware bugs as well
> as hardware faults. When a test fails in a way its page doesn't explain, save the
> terminal output and report it.

---

## 1. The hardware

```
                          USB (power, flashing, logs, serial data)
   laptop  ───────────────────────────────┐
                                          │
                        ┌─────────────────┴──────────────────┐
                        │  B-L475E-IOT01A Discovery board    │
                        │  STM32L475VG + ST-LINK + LSM6DSL   │
                        └──┬─────────┬───────────────┬───────┘
                 3V3, GND  │   D14 SDA, D15 SCL      │ A0 A1 A2 A3
                           │         │               │
   ┌───────────────────────┴─────────┴───────────────┴────────────────────┐
   │ breadboard                                                           │
   │   TCA9548A multiplexer (0x70)          piezo front end ×4 + VMID     │
   └─────┬──────────────────────┬───────────────────┬─────────────────────┘
         │ channels 0–7         │ trunk             │ 4 twisted pairs
         │ 8 × 4 wires, 30 cm   │ 4 wires           │
   ┌─────┴──────────────────────┴───────────────────┴─────────────────────┐
   │ acrylic plate 30 × 20 cm, 2 mm, on four foam feet                    │
   │   I0–I7 (BMI160, 0x68)   I8 (BMI160, 0x69)   P0–P3 (piezo discs)     │
   └──────────────────────────────────────────────────────────────────────┘
```

| Part | Role | What to know |
|---|---|---|
| **B-L475E-IOT01A** Discovery board | Microcontroller (STM32L475VG) and built-in ST-LINK programmer | One micro-USB cable gives power, flashing, the log and a serial port. Green LED2 and the blue button are used by the tests; the black button is reset |
| **GY-BMI160** modules, 9 + 1 spare | Accelerometers I0–I8 glued to the plate | Talk I2C. Two possible addresses, set by the SAO pin: 0x68 (I0–I7) or 0x69 (I8). Run at 400 samples/s, ±16 g |
| **TCA9548A** multiplexer module | Lets eight sensors with the same address share one bus | Address 0x70. Writing bit *k* connects channel *k* to the bus |
| **Piezo discs** Φ27 mm, 4 + 2 spare | Fast vibration sensors P0–P3 | Produce a voltage when the plate flexes. Sampled at 100 000 samples/s each on A0–A3 |
| **Piezo front end** (resistors, capacitors, 1N4148 diodes) | Shifts the piezo signal to mid-supply and limits it to what the ADC pin tolerates | Built on the breadboard; circuit in [tests/t09_piezo_dc.md](tests/t09_piezo_dc.md) |
| **LSM6DSL** (on the Discovery board) | Table reference: sees vibration of the table, not the plate | Nothing to wire |
| Acrylic plate | The sensing surface | 2 mm acrylic cracks: never drill it, knock with a fingertip or knuckle |

**Names used in every page**

- **D14 / D15**: the Arduino-header pins carrying I2C1 SDA / SCL. **A0–A3**: the
  analog inputs. **3V3 / GND**: the power pins on the same headers. All are
  printed on the board's silk screen next to the sockets.
- **Trunk**: the D14/D15 pair before the multiplexer. The multiplexer and I8
  both sit on it.
- **Channel *k***: the multiplexer's `SDk` / `SCk` pair, wired to sensor I*k*.
- **I0–I8**: sensors by position on the plate (proposal §2). **Module 0–9**: the
  number you tape on each module. Module *k* becomes sensor I*k*; module 9 is
  the spare.
- **LED2**: the green LED. **User button**: blue. **Reset**: black.

---

## 2. Installing the tools

Written for Linux. Where macOS or Windows differ it is noted, but those notes are
untested.

### 2.1 Get the repository

```sh
git clone https://github.com/victorwk93812/knock-localization.git
cd knock-localization
```

### 2.2 Rust

1. Install `rustup` from <https://rustup.rs> (or your distribution's `rustup`
   package). Do not install a distribution `rust` or `cargo` package beside it.
2. In the repository:

   ```sh
   rustup toolchain install
   ```

   This reads `rust-toolchain.toml` and installs exactly Rust 1.99.1 together with
   the `thumbv7em-none-eabihf` target (the compiler back end for the STM32's
   Cortex-M4F). Nothing else needs to be chosen; never edit that file.
3. Check: `cd firmware && cargo build --release --bins` finishes without errors.
   The first build downloads the dependencies and takes a few minutes.

No separate ARM GCC, OpenOCD or ST software is needed.

### 2.3 probe-rs (flashing and logs)

`probe-rs` talks to the ST-LINK on the board: it flashes the program and prints
its log. The project is developed with **probe-rs 0.32**.

1. Install it by one of the methods on
   <https://probe.rs/docs/getting-started/installation/> (prebuilt installer, or
   `cargo install probe-rs-tools --locked`).
2. **Linux only: allow access without `sudo`.** Follow "udev rules" on
   <https://probe.rs/docs/getting-started/probe-setup/>: copy `69-probe-rs.rules`
   to `/etc/udev/rules.d/`, then

   ```sh
   sudo udevadm control --reload && sudo udevadm trigger
   ```

   and replug the board.
3. Check, with the board plugged in:

   ```sh
   probe-rs list
   ```

   must show one `STLink V2-1` probe.

Windows: if the probe is not listed, install ST's ST-LINK USB driver.

### 2.4 Serial port

The same USB cable also provides a serial port (the "virtual COM port"), used for
the recorded data.

| System | Port name | Permission |
|---|---|---|
| Linux | `/dev/ttyACM0` | Your user must be in the group that owns the port: `ls -l /dev/ttyACM0` shows it (`uucp` on Arch, `dialout` on Debian/Ubuntu). `sudo usermod -aG <group> $USER`, then log out and in |
| macOS | `/dev/cu.usbmodem…` | none |
| Windows | `COM3` or similar (Device Manager) | none |

The pages use `/dev/ttyACM0`; substitute yours.

### 2.5 Python tools

The analysis script needs Python 3.11 or newer with pyserial, numpy and
matplotlib. [uv](https://docs.astral.sh/uv/) installs all of that into a private
environment:

1. Install uv (<https://docs.astral.sh/uv/getting-started/installation/>).
2. In the repository:

   ```sh
   cd host
   uv sync
   ```
3. Check: `uv run bench/capture.py --help` prints the options, and
   `uv run pytest` reports all tests passed.

### 2.6 First contact

Plug in only the Discovery board and run the board self-check:

```sh
cd firmware
cargo run --release --bin t00_board
```

Details and expected output: [tests/t00_board.md](tests/t00_board.md). If this
passes, the installation is complete.

---

## 3. How every test works

**Run** from `firmware/`:

```sh
cargo run --release --bin <name>
```

This builds the test, flashes it, and prints its log in the same terminal until
you press Ctrl-C. The program keeps running on the board after Ctrl-C. To run the
same test again, run the command again (or press the black reset button; the log
then only shows if `cargo run` is still attached).

**The log** has the same shape in every test:

```text
=== T02 mux scan ===
PASS: mux answers at 0x70
FAIL: ch3: nothing at 0x68: check this channel's SDA/SCL/power ...
INFO: verdict: failing channels: 3
RESULT: FAIL (1 failed)
```

`PASS` / `FAIL` lines are checks, `INFO` lines are measurements to write down, and
the last line is the verdict. After it, **LED2 blinks slowly (1 Hz) for a pass and
fast (5 Hz) for a fail**, so the result is visible without the terminal.

**Options** are cargo features, written after the binary name:

| Option | Effect | Tests |
|---|---|---|
| `--features knock-bench/quick` | Shortens the test to about 10 s. Use it for a first smoke run of each long test | t03–t07, t09, t10, t13 |
| `--features knock-bench/i2c-100k` | Sensor bus at 100 kHz, not 400 kHz | fallback for t03; t04 and t13 are then expected to fail (the bus is too slow to keep up) |
| `--features knock-bench/long-window` | 500 ms capture per knock | t10 only (this is test T11) |

**Tests that record data** (t06, t07, t09, t10) also send it over the serial port.
Use two terminals, in this order:

```sh
# terminal 1, first:
cd host
uv run bench/capture.py --port /dev/ttyACM0 --out ../data/bench

# terminal 2, second:
cd firmware
cargo run --release --bin t07_capture
```

**Start the capture script first.** Flashing restarts the board, and the data
header is sent right after the restart; a script started later may save nothing
(always true for t07, see [STATUS.md](STATUS.md)). The script saves each run as
`<test>-<timestamp>.csv`, prints a summary, and writes plots as PNG beside it. Stop
it with Ctrl-C. A saved file can be analysed again any time:

```sh
uv run bench/capture.py --replay ../data/bench/<file>.csv
```

`data/` is git-ignored. Write the file name next to the checklist line.

**Only one program can use the serial port at a time**, and only one can use the
probe. Close other serial monitors and IDE debug sessions.

---

## 4. Order of work

Add one thing at a time. When a test fails, the fault is in the thing just added.

| Step | Page | Needs |
|---|---|---|
| 1 | [materials.md](materials.md) §1–§3 | multimeter; parts loose |
| 2 | [t00_board](tests/t00_board.md) | Discovery board only |
| 3 | [materials.md](materials.md) §4: solder and strap the modules | soldering iron |
| 4 | [t01_chip_id](tests/t01_chip_id.md) (T1) | one module at a time, 4 wires |
| 5 | [t02_mux_scan](tests/t02_mux_scan.md) (T2) | multiplexer + 9 modules on the breadboard |
| 6 | [t03_bus_soak](tests/t03_bus_soak.md) (T3) | same, with the final 30 cm wires |
| 7 | [t04_throughput](tests/t04_throughput.md) (T4, T14) | same |
| 8 | [t05_odr_sat](tests/t05_odr_sat.md) (T5) | same; rate check now, knock check after mounting |
| 9 | [t06_noise](tests/t06_noise.md) (T6) | same; repeat after mounting |
| 10 | [materials.md](materials.md) §5: solder the piezo leads; [t09_piezo_dc](tests/t09_piezo_dc.md) (T9) | piezo front end on the breadboard |
| 11 | [t07_capture](tests/t07_capture.md) as T8 | two modules on an acrylic offcut |
| 12 | Glue everything to the plate (proposal §3.4), re-run t02 and t06 | |
| 13 | [t07_capture](tests/t07_capture.md) as T7 | assembled plate |
| 14 | [t10_piezo_time](tests/t10_piezo_time.md) (T10, T11) | assembled plate |
| 15 | [t13_recovery](tests/t13_recovery.md) (T13) | assembled plate |

Steps 1–10 are milestone M1, 11–14 are M2, 15 belongs to the rehearsal (M6).

**T12 (latency and false triggers)** has no test binary: it needs the real
acquisition firmware and the avatar. Method when those exist: film the knock and
the screen with a phone at 240 frames/s and count frames (about 4 ms each); count
triggers over 10 minutes idle, 10 minutes with someone typing on the table, and
with deliberate table bumps.

---

## 5. Multimeter habits

- Probe tips are enough; no banana-to-Dupont lead is needed. For a female Dupont
  socket, push a male header pin in and probe the pin.
- For current, the meter goes in series with the red lead in the mA jack. **Move
  the lead back to V/Ω afterwards.** Probing a supply with the lead in the mA jack
  shorts it.
- Change wiring with the USB cable unplugged.
