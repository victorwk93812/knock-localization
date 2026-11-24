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

Four things are needed: **git**, **Rust** (with the STM32 target), **probe-rs**
(flashing and logs) and **uv** (Python tools). Follow the column for your system,
then do the common steps in §2.4–§2.6.

> Developed and checked on Linux only. The Windows and macOS steps follow the
> tools' official instructions but have not been run by us yet. If a step fails,
> note the exact command and error message.

### 2.1 Windows 10 / 11

Use **PowerShell** for everything (not WSL: passing the USB probe into WSL is
extra work and not covered here).

1. **Git:** `winget install --id Git.Git -e`, or the installer from
   <https://git-scm.com>.
2. **C++ build tools (needed by Rust).** Rust on Windows needs Microsoft's linker
   and the Windows SDK, even though our program is built for the STM32: parts of
   the build run on the PC. Either
   - let the Rust installer in the next step install them when it offers to, or
   - install "Build Tools for Visual Studio 2022" from
     <https://visualstudio.microsoft.com/downloads/> and select the workload
     **Desktop development with C++**.
3. **Rust:** download and run `rustup-init.exe` from <https://rustup.rs>
   (64-bit: <https://win.rustup.rs/x86_64>; ARM64 PC:
   <https://win.rustup.rs/aarch64>). Accept the defaults. **Open a new PowerShell
   window afterwards** so `cargo` is on the PATH.
4. **probe-rs:**

   ```powershell
   irm https://github.com/probe-rs/probe-rs/releases/latest/download/probe-rs-tools-installer.ps1 | iex
   ```

   If PowerShell refuses to run the script, start it with
   `powershell -ExecutionPolicy ByPass` and repeat.
5. **ST-LINK driver.** probe-rs needs ST's official USB driver on Windows:
   download **STSW-LINK009** from st.com (free account) and install it, then
   replug the board. Do **not** use Zadig to replace the driver.
6. **uv:**

   ```powershell
   powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 | iex"
   ```

   (or `winget install --id=astral-sh.uv -e`). Open a new PowerShell window.
7. **Serial port name:** open Device Manager → Ports (COM & LPT) with the board
   plugged in. "STMicroelectronics STLink Virtual COM Port (COM*n*)" gives the
   name, for example `COM5`. Use that wherever the pages say `/dev/ttyACM0`.

Differences to keep in mind on Windows:

- Paths in the commands work as written (`../data/bench` is fine in PowerShell).
- Where a command is shown over several lines ending in `\`, type it on one line
  (or end the lines with a backtick `` ` ``).
- In the t00 echo check, quit with Ctrl-Z then Enter (not Ctrl-D).
- Some antivirus tools slow the first build a lot; it is not hung.

### 2.2 macOS (Intel or Apple Silicon)

Use Terminal.

1. **Command line tools (git and the linker Rust needs):**

   ```sh
   xcode-select --install
   ```
2. **Rust:**

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   Accept the defaults, then open a new terminal window.
3. **probe-rs:**

   ```sh
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/probe-rs/probe-rs/releases/latest/download/probe-rs-tools-installer.sh | sh
   ```

   (or, with Homebrew: `brew tap probe-rs/probe-rs && brew install probe-rs`).
   No driver is needed for the ST-LINK.
4. **uv:**

   ```sh
   curl -LsSf https://astral.sh/uv/install.sh | sh
   ```

   (or `brew install uv`). Open a new terminal window.
5. **Serial port name:** with the board plugged in, `ls /dev/cu.usbmodem*`. Use
   that name (for example `/dev/cu.usbmodem1103`) wherever the pages say
   `/dev/ttyACM0`. Use the `cu.` name, not the `tty.` one.

### 2.3 Linux

1. **Git and build basics** from your distribution (`git`, a C compiler/linker
   such as `gcc`). On Debian/Ubuntu also `sudo apt install -y pkg-config
   libudev-dev`.
2. **Rust:**

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

   (or your distribution's `rustup` package). Do not install a distribution
   `rust` or `cargo` package beside it.
3. **probe-rs:**

   ```sh
   curl --proto '=https' --tlsv1.2 -LsSf https://github.com/probe-rs/probe-rs/releases/latest/download/probe-rs-tools-installer.sh | sh
   ```
4. **Probe access without `sudo`:** download `69-probe-rs.rules` from
   <https://probe.rs/docs/getting-started/probe-setup/>, copy it to
   `/etc/udev/rules.d/`, then

   ```sh
   sudo udevadm control --reload && sudo udevadm trigger
   ```

   and replug the board.
5. **uv:** `curl -LsSf https://astral.sh/uv/install.sh | sh`
6. **Serial port:** `/dev/ttyACM0`. Your user must be in the group that owns it:
   `ls -l /dev/ttyACM0` shows the group (`uucp` on Arch, `dialout` on
   Debian/Ubuntu). `sudo usermod -aG <group> $USER`, then log out and in.

### 2.4 Get the repository and the pinned toolchain (all systems)

```sh
git clone https://github.com/victorwk93812/knock-localization.git
cd knock-localization
rustup toolchain install
```

The last command reads `rust-toolchain.toml` and installs exactly Rust 1.99.1
together with the `thumbv7em-none-eabihf` target (the compiler back end for the
STM32's Cortex-M4F). Never edit that file. If your rustup is old and asks for a
toolchain name, run `rustup update` first, or simply continue: the first `cargo`
command in the repository installs the pinned toolchain by itself.

No ARM GCC, OpenOCD or STM32Cube software is needed.

### 2.5 Check the installation (all systems)

| Check | Command | Expect |
|---|---|---|
| Rust | `cargo --version` inside the repository | `cargo 1.99.1` |
| Firmware builds | `cd firmware` then `cargo build --release --bins` | `Finished`, no errors. The first build downloads dependencies and takes a few minutes |
| probe-rs | `probe-rs --version` | 0.32 or newer |
| Probe | board plugged in, `probe-rs list` | one `STLink V2-1` probe |
| Python tools | `cd host`, `uv sync`, then `uv run bench/capture.py --help` | the option list. uv downloads Python 3.11+ by itself if needed |
| Script tests | in `host`: `uv run pytest` | all passed |

If the probe is not listed:

- **Any system:** try another USB cable (many are charge-only) and the board's
  **ST-LINK** USB socket; close other programs using the probe (STM32CubeIDE,
  another terminal still running `cargo run`).
- **Windows:** the ST driver from §2.1 step 5 is missing.
- **Linux:** the udev rules from §2.3 step 4 are missing.
- **Probe listed but flashing fails with a firmware-version message:** the
  ST-LINK's own firmware is older than 2.26. Update it with ST's updater
  (STSW-LINK007).

### 2.6 First contact

Plug in only the Discovery board and run the board self-check:

```sh
cd firmware
cargo run --release --bin t00_board
```

Details and expected output: [tests/t00_board.md](tests/t00_board.md). If this
passes and the serial echo check on that page works, the installation is
complete.

Only for people changing the driver code: the unit tests in `firmware/README.md`
are written with `--target x86_64-unknown-linux-gnu`. On another system use your
own host target, shown by `rustc -vV` in the `host:` line (for example
`aarch64-apple-darwin` or `x86_64-pc-windows-msvc`).

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
