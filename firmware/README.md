# Firmware

Rust firmware for the knock-localization plate on the B-L475E-IOT01A Discovery
kit (STM32L475VG), built on [Embassy](https://embassy.dev).

| Crate | What it is |
|---|---|
| `knock-drivers` | `#![no_std]` sensor drivers and signal helpers. No HAL dependency; unit-tested on the host. |
| `knock-bench` | Board support, test-report helpers and the bench-test binaries (`src/bin/tNN_*.rs`). |

Status: everything here compiles and the `knock-drivers` tests pass on the host.
**Nothing has been run on hardware yet.**

## Build and test

Run from `firmware/`. The toolchain is pinned by `../rust-toolchain.toml`; the
default target (`thumbv7em-none-eabihf`) is set in `.cargo/config.toml`.

```sh
cargo build --release --bins                                    # all firmware binaries
cargo test -p knock-drivers --target x86_64-unknown-linux-gnu   # host unit tests
cargo clippy --release --bins -- -D warnings
cargo clippy -p knock-drivers --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
cargo fmt --check
```

I2C1 runs at 400 kHz. To build for 100 kHz (the T3 fallback):

```sh
cargo build --release --bins --features knock-bench/i2c-100k
```

Flashing uses the runner in `.cargo/config.toml`
(`cargo run --release --bin t00_board` → `probe-rs run --chip STM32L475VG`).
Logs come back over RTT through defmt at `info` level (`DEFMT_LOG` in the same
file).

## Bench tests

| Binary | Test | Wiring | Checks |
|---|---|---|---|
| `t00_board` | board bring-up | Discovery kit only | LED2, VCP banner + echo (921600 8N1), LSM6DSL WHO_AM_I and gravity; I2C1 scan (informational) |
| `t01_chip_id` | T1 | one BMI160 module directly on I2C1 | address (SAO strap), CHIP_ID = 0xD1, configuration, 1 s of samples: mean, gravity 0.9–1.1 g, RMS noise. User button = next module (0–9); hold 2 s to finish |
| `t02_mux_scan` | T2 | TCA9548A + I0..I7 + I8 | mux at 0x70, per-channel readback / scan / CHIP_ID, 0x68 absent with all channels off, I8 at 0x69 |
| `t03_bus_soak` | T3 | full array | 1 000 000 transfers round-robin (mux select, CHIP_ID, 6 data bytes); per-sensor NACK / timeout / wrong-ID / other counters; pass = 0 errors. Also with `--features knock-bench/i2c-100k` |
| `t04_throughput` | T4 (load for T14) | full array; logic analyzer on Arduino D2 (PD14) optional | drain all FIFOs every 20 ms for 10 min: loop time min/avg/max, max FIFO level, overflows, frames vs expected, I2C errors |
| `t05_odr_sat` | T5 | full array on the plate | 10 s still: frames/s per sensor within 400 ± 2 %; then knock hard until the user button: peak \|a\| per sensor in g, any axis ≥ 15.5 g flagged |
| `t06_noise` | T6 | full array on the plate + onboard LSM6DSL | 60 s still: mean / RMS per sensor and axis in mg, 5σ threshold suggestion, sensors > 3× median flagged; CSV summary |
| `t07_capture` | T7, T8 | full array, or any subset (no mux: 0x68 and 0x69 on the trunk) | knock capture at 300 mg: −4…+15 samples per sensor as CSV, peak A_i per sensor; user button ends the run |
| `t13_recovery` | T13 | full array | 30 min acquisition while sensors are unplugged / replugged: outage log, re-init, bus recovery, IWDG watchdog |

`t03`–`t07` and `t13` take `--features knock-bench/quick`, which shortens them
to about 10 s for a smoke run. They share `knock_bench::imu` (array bring-up
and round-robin FIFO readout).

Every test prints the same report over defmt and then idles blinking LED2:
slowly (1 Hz) on PASS, fast (5 Hz) on FAIL.

```text
=== T02 mux scan ===
PASS: mux answers at 0x70
FAIL: ch3: nothing at 0x68: check this channel's SDA/SCL/power ...
INFO: verdict: failing channels: 3
RESULT: FAIL (1 failed)
```

## `knock-drivers`

The drivers do not own the bus. Each one is a small `Copy` handle holding the
I2C address, and every method takes `&mut impl embedded_hal_async::i2c::I2c`.
That is what lets nine BMI160s and the multiplexer share one bus without a
mutex.

| Module | Contents |
|---|---|
| `Accel` (crate root) | One raw sample `{ x, y, z: i16 }`; `from_le_bytes`, `to_array`, `magnitude`. |
| `bmi160` | `Bmi160::new(addr)`; `chip_id`, `check_chip_id`, `soft_reset`, `init_accel_fifo(i2c, delay)`, `read_accel`, `fifo_len`, `read_fifo`, `read_fifo_frames`, `sensortime`, `flush_fifo`, `error_status`, `pmu_status`, `suspend_accel`; register constants in `bmi160::reg` / `bmi160::cmd`; `raw_to_mg_16g`, `sensortime_to_us`. |
| `tca9548a` | `Tca9548a::new(addr)`; `select(mask)`, `select_channel(k)`, `readback`, `disable_all`. |
| `lsm6dsl` | `Lsm6dsl::new(addr)`; `who_am_i`, `init_accel` (416 Hz, ±16 g), `read_accel`; `raw_to_mg_16g`. |
| `fifo` | `frames(&bytes)` iterates headerless 6-byte frames as `Accel`; `.remainder()` is the trailing partial frame. |
| `stats` | `mean`, `rms`, `std_dev`, `peak_abs`, `peak_abs_dev`, `magnitude`, `onset_index(samples, baseline_len, k_sigma)`; generic over `i16` and `f32` slices. |
| `scan` | `probe(i2c, addr)`, `scan(i2c)` (0x08–0x77), `scan_range`; result type `AddressSet`. |
| `recovery` | `recover_bus(scl, sda, delay, half_period_us)`: 9 SCL pulses + STOP on two open-drain pins; returns `BusState`. |
| `health` | `ErrorClass`, `ErrorCounters` (per-sensor NACK / timeout / wrong-ID / other), `OutageTracker` (healthy ↔ down state machine), `GapTracker`. |
| `rate` | `RateMeter` (frames/s from FIFO bursts), `expected_frames`, `within_percent`, `OverflowDetector` (FIFO level ≥ 1000 bytes or a readout gap longer than the FIFO holds). |
| `running` | Streaming statistics: `Accum`, `AccelAccum` (mean, RMS, dynamic RMS without storing samples), `MinMaxMean`, `median`. |
| `knock` | Knock capture: `SampleRing` (ring buffer with sample numbers and time anchor), `Baseline`, `Earliest`, `Refractory`, `extract_window`, `peak_dynamic`, `argmax`. |

`init_accel_fifo` sets the project configuration: soft reset, accelerometer
normal mode (gyroscope stays suspended), 400 Hz normal filter, ±16 g,
accelerometer-only headerless FIFO, FIFO flushed. Typical readout:

```rust
let n = imu.read_fifo_frames(&mut i2c, &mut buf).await?; // bounded by FIFO_LENGTH
for sample in fifo::frames(&buf[..n]) {
    let z_mg = bmi160::raw_to_mg_16g(sample.z);
}
```

## `knock-bench` library

| Module | Contents |
|---|---|
| `board` | `Board::init()` → `Board { i2c1, i2c2, vcp, led2, button, piezo }`; `Irqs`; `wait_for_press`; constants `I2C1_HZ`, `VCP_BAUD`, `SYSCLK_HZ`. Pin map, DMA channels and clock set-up are documented at the top of `board.rs`. |
| `bus` | `TimeoutI2c::new(bus)`: wraps a bus so a hung transfer returns `BusError::Timeout` (200 ms by default). |
| `recovery` | `recover_pins(scl, sda)`; normally reached through `I2c1Port::recover()`. |
| `report` | `Report` (`begin`, `check`, `pass`, `fail`, `finish`), macros `check!` `pass!` `fail!` `note!` `row!`, `Verdict`, `idle_blink`, `CsvWriter`. |
| `scan`, `drivers` | Re-exports of `knock_drivers::scan` and `knock_drivers`. |
| `imu` | The nine-sensor array: `bring_up(&mut port)` → `ImuArray`; `drain_round`, `drain`, `init_sensor`, `probe`, `flush_all`, `park`; `probe_pin()` (Arduino D2 = PD14), `start_watchdog`, `take_watchdog_reset_flag`. The mux policy is documented at the top of `imu.rs`. |

### I2C1 and bus recovery

`Board::i2c1` is an `I2c1Port`, not a bus. The port owns the I2C1 peripheral,
its pins and DMA channels, and lends them out:

- `port.bus()` returns a freshly initialised `I2c<'_>` borrowing the port;
- `port.recover().await` runs bus recovery on PB8/PB9 as GPIOs and reports a
  `BusState`. It needs `&mut port`, so the bus must be dropped first.

```rust
let mut port = board.i2c1;
let outcome = {
    let mut i2c = TimeoutI2c::new(port.bus());
    run(&mut i2c).await
}; // the bus is dropped here
if outcome.is_err() {
    let state = port.recover().await;
}
```

`Board::i2c2` (onboard sensors) is a plain `I2c<'static>`.

### Piezo resources

`Board::piezo` hands out, unconfigured: `adc` (ADC1), `adc_dma` (DMA1_CH1) and
the pins `a0..a3` = PC5, PC4, PC3, PC2 = ADC1_IN14, IN13, IN4, IN3. The ADC
kernel clock is already set to SYSCLK (80 MHz). `Irqs` does not bind `ADC1_2` or
`DMA1_CHANNEL1`; declare a second `bind_interrupts!` struct for those where they
are used. The embassy time driver (`time-driver-any`) takes TIM15 on this chip,
so use a different timer for ADC triggering.

### CSV output on the virtual COM port

```rust
let (tx, _rx) = board.vcp.split();
let mut csv = CsvWriter::new(tx);
csv.header("t05", &["t_ms", "sensor", "x", "y", "z"]).await?;
csv.row(format_args!("{},{},{},{},{}", t, k, a.x, a.y, a.z)).await?;
csv.end().await?;
```

produces `# t05,t_ms,sensor,x,y,z`, the data rows, then `# end`. Lines end in
`\n` and are limited to 192 bytes.

## Adding a test binary

Create `knock-bench/src/bin/tNN_name.rs`; Cargo picks it up automatically.

```rust
//! TNN: what this test proves, the wiring it needs, the pass criteria.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use knock_bench::board::Board;
use knock_bench::bus::TimeoutI2c;
use knock_bench::report::{self, Report};
use knock_bench::{check, note};

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board { i2c1: mut port, mut led2, .. } = Board::init();
    let mut report = Report::begin("TNN", "short name");

    {
        let mut i2c = TimeoutI2c::new(port.bus());
        // ... run the checks ...
        check!(report, true, "what was checked: {}", 42);
        note!("extra detail, not a check");
    }

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
```

Linking `knock_bench` brings in the defmt RTT logger and the panic handler.
Sensor-level logic that does not need the HAL belongs in `knock-drivers`, where
it can be tested on the host with `embedded-hal-mock`.

## Not verified yet

- The LSM6DSL register values were checked against ST's driver header, not the
  datasheet; its ±16 g sensitivity (0.488 mg/LSB) is from memory.
- The Arduino-header pin nets were checked against the Zephyr board files, not
  UM2153.
- The BMI160 soft-reset wait (10 ms) is a conservative choice; the datasheet
  gives no figure.
- "Arduino D2 = PD14" (the logic-analyzer pin of `t04`) is from the Zephyr board
  files as remembered, not UM2153.
- Search the sources for `UNVERIFIED` for the exact places.
