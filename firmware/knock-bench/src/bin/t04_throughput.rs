//! T4: round-robin throughput (also the steady load for the T14 current
//! measurement).
//!
//! Wiring: the full array (mux 0x70, I0..I7 behind it, I8 at 0x69 on the
//! trunk). Optional: a logic analyzer on **Arduino D2 (PD14)**, which is high
//! while a readout round is running and low while the loop waits, so the
//! pulse width is the loop time and the period is 20 ms.
//!
//! All nine sensors run in the project configuration (400 Hz, ±16 g,
//! headerless FIFO). Every 20 ms all FIFOs are drained, for 10 minutes. A
//! summary is printed every 60 s and a per-sensor table at the end:
//!
//! - loop time (duration of one readout round) min / avg / max in µs, measured
//!   with `embassy_time::Instant` (1 µs ticks);
//! - highest FIFO fill level per sensor in frames (8 expected, 170 = full);
//! - FIFO overflows (fill level ≥ 1000 bytes, or a readout gap longer than the
//!   FIFO can hold);
//! - frames received per sensor against 400 Hz × elapsed time;
//! - I2C errors per sensor.
//!
//! Pass: all nine sensors configured, no FIFO overflow, maximum loop time
//! below 20 ms, no I2C error.
//!
//! Nothing to do while it runs; leave the plate alone. Build with
//! `--features knock-bench/quick` for a 10 s smoke run. Ends with the RESULT
//! line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker};
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::bmi160;
use knock_bench::drivers::health::ErrorCounters;
use knock_bench::drivers::rate::{self, OverflowDetector, RateMeter};
use knock_bench::drivers::running::MinMaxMean;
use knock_bench::imu::{self, ROUND_PERIOD, SENSORS};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, row};

/// Length of the run.
const DURATION: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(10)
} else {
    Duration::from_secs(600)
};
/// Interval between summary lines.
const SUMMARY_EVERY: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(5)
} else {
    Duration::from_secs(60)
};
/// Give up after this many bus recoveries: the bus is not usable.
const MAX_RECOVERIES: u32 = 20;
/// Frame counts further than this from 400 Hz × time are pointed out
/// (BMI160 output data rate accuracy ±1 % plus the MCU's HSI16 clock ±1 %).
const RATE_TOLERANCE_PERCENT: f32 = 2.0;

/// What is tracked per sensor.
#[derive(Clone, Copy)]
struct SensorStats {
    errors: ErrorCounters,
    overflow: OverflowDetector,
    rate: RateMeter,
}

impl SensorStats {
    const fn new() -> Self {
        Self {
            errors: ErrorCounters::new(),
            overflow: OverflowDetector::new(bmi160::ODR_HZ),
            rate: RateMeter::new(),
        }
    }
}

fn total_overflows(stats: &[SensorStats; SENSORS]) -> u32 {
    stats.iter().map(|s| s.overflow.overflows()).sum()
}

fn total_errors(stats: &[SensorStats; SENSORS]) -> u32 {
    stats.iter().map(|s| s.errors.errors()).sum()
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        ..
    } = Board::init();
    let mut probe = imu::probe_pin();

    let mut report = Report::begin("T04", "throughput");
    note!(
        "I2C1 at {} Hz; drain every {} ms for {} s; logic analyzer: Arduino D2 (PD14) high during each round",
        board::I2C1_HZ,
        ROUND_PERIOD.as_millis(),
        DURATION.as_secs()
    );

    let mut array = imu::bring_up(&mut port).await;
    check!(
        report,
        array.ready_count() == SENSORS,
        "{} of {} sensors configured",
        array.ready_count(),
        SENSORS
    );
    if array.ready_count() == 0 {
        let verdict = report.finish();
        report::idle_blink(&mut led2, verdict).await
    }

    let mut stats = [SensorStats::new(); SENSORS];
    let mut loop_us = MinMaxMean::new();
    let mut recoveries = 0u32;
    let mut gave_up = false;
    let mut flushed = false;
    let mut started = Instant::now();
    let mut next_summary = SUMMARY_EVERY;

    'run: loop {
        {
            let mut i2c = TimeoutI2c::new(port.bus());
            if !flushed {
                // Start with empty FIFOs: bring-up took long enough to fill
                // them with a few dozen frames.
                if let Err((k, fault)) = array.flush_all(&mut i2c).await {
                    note!("I{}: FIFO flush failed: {:?}", k, fault);
                }
                flushed = true;
                started = Instant::now();
            }
            let mut ticker = Ticker::every(ROUND_PERIOD);
            loop {
                ticker.next().await;
                let elapsed = started.elapsed();
                if elapsed >= DURATION {
                    array.park(&mut i2c).await;
                    break 'run;
                }

                probe.set_high();
                let round = array
                    .drain_round(&mut i2c, |k, result| {
                        let s = &mut stats[k];
                        match result {
                            Ok(drain) => {
                                let at_us = drain.at.as_micros();
                                s.errors.record_ok();
                                s.overflow.observe(at_us, drain.level_bytes);
                                s.rate.burst(at_us, drain.frames.len() as u32);
                            }
                            Err(fault) => s.errors.record(fault.class()),
                        }
                    })
                    .await;
                probe.set_low();
                loop_us.push(round.took.as_micros());

                if elapsed >= next_summary {
                    next_summary += SUMMARY_EVERY;
                    led2.toggle();
                    let mut levels = [0u16; SENSORS];
                    for (level, s) in levels.iter_mut().zip(&stats) {
                        *level = s.overflow.max_level_frames();
                    }
                    note!(
                        "t = {} s: loop min/avg/max = {}/{}/{} us, max FIFO level {:?} frames, {} overflows, {} I2C errors",
                        elapsed.as_secs(),
                        loop_us.min(),
                        loop_us.mean(),
                        loop_us.max(),
                        levels,
                        total_overflows(&stats),
                        total_errors(&stats)
                    );
                }
                if round.aborted.is_some() {
                    break;
                }
            }
        } // the bus is dropped here

        recoveries += 1;
        let state = port.recover().await;
        note!("bus recovery {}: {:?}", recoveries, state);
        if recoveries > MAX_RECOVERIES {
            gave_up = true;
            break;
        }
    }
    probe.set_low();
    led2.set_low();

    row!("sensor | frames | expected | dev %  | max level | overflows | I2C errors");
    for (k, s) in stats.iter().enumerate() {
        if !array.is_ready(k) {
            row!("   I{}  | not configured", k);
            continue;
        }
        let expected = rate::expected_frames(s.rate.span_us(), bmi160::ODR_HZ);
        let hz = s.rate.rate_hz().unwrap_or(0.0);
        let dev = rate::deviation_percent(hz, bmi160::ODR_HZ as f32);
        row!(
            "   I{}  | {:>6} | {:>8} | {:>+6.2} | {:>9} | {:>9} | {:>10}{}",
            k,
            s.rate.frames(),
            expected,
            dev,
            s.overflow.max_level_frames(),
            s.overflow.overflows(),
            s.errors.errors(),
            if rate::within_percent(hz, bmi160::ODR_HZ as f32, RATE_TOLERANCE_PERCENT) {
                ""
            } else {
                "  <- frame count outside +-2 %"
            }
        );
    }
    note!(
        "loop time min/avg/max = {}/{}/{} us over {} rounds",
        loop_us.min(),
        loop_us.mean(),
        loop_us.max(),
        loop_us.count()
    );

    check!(
        report,
        total_overflows(&stats) == 0,
        "{} FIFO overflows",
        total_overflows(&stats)
    );
    check!(
        report,
        loop_us.count() != 0 && loop_us.max() < ROUND_PERIOD.as_micros(),
        "maximum loop time {} us (limit {} us)",
        loop_us.max(),
        ROUND_PERIOD.as_micros()
    );
    check!(
        report,
        total_errors(&stats) == 0 && recoveries == 0,
        "{} I2C errors, {} bus recoveries",
        total_errors(&stats),
        recoveries
    );
    if gave_up {
        fail!(
            report,
            "run abandoned after {} bus recoveries: the bus kept hanging",
            recoveries
        );
    }

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
