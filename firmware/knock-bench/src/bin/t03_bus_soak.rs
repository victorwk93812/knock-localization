//! T3: I2C bus integrity soak.
//!
//! Wiring: the full array: TCA9548A (0x70) on I2C1, BMI160 I0..I7 (0x68) on
//! its channels 0..7, BMI160 I8 (0x69) on the trunk.
//!
//! Makes 1 000 000 bus transfers in total, round-robin over the nine sensors.
//! One visit to a sensor is: select its multiplexer channel (one write,
//! counted; not needed for I8), read CHIP_ID (must be 0xD1), read the six
//! acceleration data bytes. A round over all nine sensors is therefore 26
//! transfers. Every transfer is counted against the sensor being visited as
//! OK, NACK, timeout, wrong CHIP_ID or other error. If a channel select fails,
//! the two reads of that visit are skipped (they would reach the wrong
//! sensor).
//!
//! A progress line is printed every 50 000 transfers. After a timeout the bus
//! is recovered (9 SCL pulses + STOP) and the soak continues; if the bus has
//! to be recovered more than `MAX_RECOVERIES` times the soak is abandoned.
//!
//! Pass: no error of any kind on any sensor.
//!
//! Nothing to do while it runs; do not touch the wiring. Build with
//! `--features knock-bench/i2c-100k` for the 100 kHz fallback, and with
//! `--features knock-bench/quick` for a 20 000-transfer smoke run. Ends with
//! the RESULT line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::Instant;
use embedded_hal_async::i2c::I2c;
use knock_bench::board::{self, Board};
use knock_bench::bus::{BusError, TimeoutI2c};
use knock_bench::drivers::bmi160;
use knock_bench::drivers::health::{ErrorClass, ErrorCounters};
use knock_bench::imu::{self, SENSORS, TRUNK_SENSOR};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, row};

/// Total number of bus transfers.
const TOTAL_READS: u32 = if cfg!(feature = "quick") {
    20_000
} else {
    1_000_000
};
/// A progress line is printed every this many transfers.
const PROGRESS_EVERY: u32 = if cfg!(feature = "quick") {
    5_000
} else {
    50_000
};
/// Give up after this many bus recoveries: the bus is not usable.
const MAX_RECOVERIES: u32 = 50;

/// The transfers of one visit to a sensor, in order.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Select,
    ChipId,
    Data,
}

/// Where the soak is in its round-robin.
#[derive(Clone, Copy)]
struct Cursor {
    sensor: usize,
    step: Step,
}

impl Cursor {
    const fn start() -> Self {
        Self {
            sensor: 0,
            step: Step::Select,
        }
    }

    /// First step of a visit to `sensor` (I8 needs no channel select).
    const fn visit(sensor: usize) -> Self {
        Self {
            sensor,
            step: if sensor == TRUNK_SENSOR {
                Step::ChipId
            } else {
                Step::Select
            },
        }
    }

    /// Advance after a transfer; `ok` is false if it failed.
    fn advance(&mut self, ok: bool) {
        *self = match (self.step, ok) {
            (Step::Select, true) => Self {
                step: Step::ChipId,
                ..*self
            },
            (Step::ChipId, _) => Self {
                step: Step::Data,
                ..*self
            },
            // Visit finished, or the channel select failed: next sensor.
            (Step::Data, _) | (Step::Select, false) => Self::visit((self.sensor + 1) % SENSORS),
        };
    }
}

/// Make the transfer the cursor points at.
async fn transfer<I, E>(i2c: &mut I, at: Cursor) -> Result<(), ErrorClass>
where
    I: I2c<Error = BusError<E>>,
    E: embedded_hal::i2c::Error,
{
    let imu = imu::sensor(at.sensor);
    match at.step {
        Step::Select => imu::MUX
            .select(i2c, 1 << at.sensor)
            .await
            .map_err(|e| imu::classify(&e)),
        Step::ChipId => match imu.chip_id(i2c).await {
            Ok(bmi160::CHIP_ID_VALUE) => Ok(()),
            Ok(_) => Err(ErrorClass::WrongChipId),
            Err(e) => Err(imu::classify(&e)),
        },
        Step::Data => imu
            .read_accel(i2c)
            .await
            .map(|_| ())
            .map_err(|e| imu::classify(&e)),
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        ..
    } = Board::init();

    let mut report = Report::begin("T03", "bus soak");
    note!(
        "I2C1 at {} Hz; {} transfers, progress every {}",
        board::I2C1_HZ,
        TOTAL_READS,
        PROGRESS_EVERY
    );

    // Informational: which sensors answer and accept the configuration. The
    // soak itself visits all nine regardless, so a missing sensor shows up as
    // NACKs in its row.
    let array = imu::bring_up(&mut port).await;
    if !array.mux_present() {
        note!("no multiplexer: every channel select will be counted as an error");
    }

    let mut counters = [ErrorCounters::new(); SENSORS];
    let mut cursor = Cursor::start();
    let mut done = 0u32;
    let mut errors = 0u32;
    let mut recoveries = 0u32;
    let mut gave_up = false;
    let started = Instant::now();

    'soak: while done < TOTAL_READS {
        {
            let mut i2c = TimeoutI2c::new(port.bus());
            loop {
                if done >= TOTAL_READS {
                    array.park(&mut i2c).await;
                    break 'soak;
                }
                let result = transfer(&mut i2c, cursor).await;
                match result {
                    Ok(()) => counters[cursor.sensor].record_ok(),
                    Err(class) => {
                        counters[cursor.sensor].record(class);
                        errors += 1;
                        // Log the first few individually; the table has the rest.
                        if errors <= 10 {
                            defmt::warn!(
                                "transfer {=u32}: I{=usize}: {}",
                                done,
                                cursor.sensor,
                                class
                            );
                        }
                    }
                }
                done += 1;
                cursor.advance(result.is_ok());

                if done.is_multiple_of(PROGRESS_EVERY) {
                    led2.toggle();
                    note!(
                        "{} / {} transfers, {} errors, {} bus recoveries, {} s",
                        done,
                        TOTAL_READS,
                        errors,
                        recoveries,
                        started.elapsed().as_secs()
                    );
                }
                if result == Err(ErrorClass::Timeout) {
                    break;
                }
            }
        } // the bus is dropped here

        recoveries += 1;
        let state = port.recover().await;
        if recoveries <= 10 {
            note!("bus recovery {}: {:?}", recoveries, state);
        }
        if recoveries > MAX_RECOVERIES {
            gave_up = true;
            break;
        }
    }
    led2.set_low();

    let elapsed = started.elapsed();
    note!(
        "{} transfers in {} s ({} per second)",
        done,
        elapsed.as_secs(),
        u64::from(done) * 1000 / elapsed.as_millis().max(1)
    );

    row!("sensor | transfers | nack | timeout | wrong id | other");
    for (k, c) in counters.iter().enumerate() {
        row!(
            "   I{}  | {:>9} | {:>4} | {:>7} | {:>8} | {:>5}",
            k,
            c.reads,
            c.nack,
            c.timeout,
            c.wrong_chip_id,
            c.other
        );
    }
    for (k, c) in counters.iter().enumerate() {
        check!(
            report,
            c.is_clean(),
            "I{}: {} errors in {} transfers",
            k,
            c.errors(),
            c.reads
        );
    }
    check!(
        report,
        recoveries == 0,
        "{} bus recoveries needed",
        recoveries
    );
    if gave_up {
        fail!(
            report,
            "soak abandoned after {} of {} transfers: the bus kept hanging (SDA or SCL held low?)",
            done,
            TOTAL_READS
        );
    } else {
        check!(
            report,
            done == TOTAL_READS,
            "{} of {} transfers made",
            done,
            TOTAL_READS
        );
    }
    if errors != 0 {
        note!(
            "errors on one sensor only: check that sensor's wires; errors on all: try --features knock-bench/i2c-100k, shorter wires, or remove duplicate pull-ups"
        );
    }

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
