//! T5: output data rate and saturation.
//!
//! Wiring: the full array, mounted on the plate.
//!
//! Phase 1 (10 s, **do not touch the plate**): counts the frames each sensor
//! delivers through its FIFO and reports frames per second. Pass if every
//! configured sensor is within 400 Hz ± 2 %. The BMI160 datasheet (table 2,
//! "output data rate accuracy", normal mode, whole temperature range) gives
//! ±1 %; the other ±1 % is the MCU's HSI16 clock, against which the rate is
//! measured.
//!
//! Phase 2 (LED2 on, until the blue user button is pressed): **knock as hard
//! as you ever will, right next to each module.** A knock is detected when any
//! sensor's dynamic acceleration exceeds 300 mg; for the following 60 ms the
//! peak |a| (vector magnitude, gravity included) of every sensor is collected
//! and printed in g, and any axis reaching 15.5 g is flagged as saturated
//! (full scale is ±16 g). Knocks closer together than 300 ms count as one.
//!
//! Pass: rates in range, at least one knock seen, no axis saturated. The last
//! lines give the worst peak seen.
//!
//! Build with `--features knock-bench/quick` to shorten phase 1 to 3 s. Ends
//! with the RESULT line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker};
use heapless::String;
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::bmi160;
use knock_bench::drivers::knock::Baseline;
use knock_bench::drivers::rate::{self, RateMeter};
use knock_bench::imu::{self, ROUND_PERIOD, SENSORS};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note};

/// Length of the still phase.
const STILL_PHASE: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(3)
} else {
    Duration::from_secs(10)
};
/// Accepted deviation of the frame rate from 400 Hz, in percent.
const RATE_TOLERANCE_PERCENT: f32 = 2.0;
/// Dynamic acceleration that starts a knock, in mg.
const TRIGGER_MG: f32 = 300.0;
/// Peaks are collected for this long after the trigger.
const EVENT_WINDOW: Duration = Duration::from_millis(60);
/// No new knock is accepted for this long after a trigger.
const REFRACTORY: Duration = Duration::from_millis(300);
/// An axis at or beyond this value counts as saturated: 15.5 g in LSB.
const SATURATION_LSB: i32 = 31 * bmi160::LSB_PER_G_16G / 2;
/// Give up after this many bus recoveries: the bus is not usable.
const MAX_RECOVERIES: u32 = 20;

const LSB_PER_G: f32 = bmi160::LSB_PER_G_16G as f32;

/// Peak values of every sensor over some stretch of samples.
#[derive(Clone, Copy)]
struct Peaks {
    /// Largest vector magnitude |a| per sensor, in LSB.
    magnitude: [f32; SENSORS],
    /// Largest absolute single-axis value per sensor, in LSB.
    axis: [i32; SENSORS],
    /// Largest dynamic acceleration of any sensor, in LSB.
    dynamic: f32,
}

impl Peaks {
    const fn new() -> Self {
        Self {
            magnitude: [0.0; SENSORS],
            axis: [0; SENSORS],
            dynamic: 0.0,
        }
    }

    fn merge(&mut self, other: &Peaks) {
        for k in 0..SENSORS {
            self.magnitude[k] = self.magnitude[k].max(other.magnitude[k]);
            self.axis[k] = self.axis[k].max(other.axis[k]);
        }
        self.dynamic = self.dynamic.max(other.dynamic);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Still,
    Knocks,
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        button,
        ..
    } = Board::init();

    let mut report = Report::begin("T05", "odr and saturation");
    note!("I2C1 at {} Hz", board::I2C1_HZ);

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

    let threshold_lsb = TRIGGER_MG * LSB_PER_G / 1000.0;
    let mut rates = [RateMeter::new(); SENSORS];
    let mut baselines = [Baseline::new(); SENSORS];
    let mut phase = Phase::Still;
    let mut flushed = false;
    let mut started = Instant::now();
    let mut i2c_errors = 0u32;
    let mut recoveries = 0u32;

    // Phase 2 state.
    let mut event: Option<(Peaks, Instant)> = None;
    let mut hold_until = Instant::MIN;
    let mut knocks = 0u32;
    let mut saturated_knocks = 0u32;
    // (peak |a| in LSB, sensor, knock number)
    let mut worst: (f32, usize, u32) = (0.0, 0, 0);

    note!(
        "phase 1: measuring the frame rate for {} s, do not touch the plate",
        STILL_PHASE.as_secs()
    );

    'run: loop {
        {
            let mut i2c = TimeoutI2c::new(port.bus());
            if !flushed {
                if let Err((k, fault)) = array.flush_all(&mut i2c).await {
                    note!("I{}: FIFO flush failed: {:?}", k, fault);
                }
                flushed = true;
                started = Instant::now();
            }
            let mut ticker = Ticker::every(ROUND_PERIOD);
            loop {
                ticker.next().await;

                match phase {
                    Phase::Still if started.elapsed() >= STILL_PHASE => {
                        for (k, meter) in rates.iter().enumerate() {
                            if !array.is_ready(k) {
                                continue;
                            }
                            let hz = meter.rate_hz().unwrap_or(0.0);
                            check!(
                                report,
                                rate::within_percent(
                                    hz,
                                    bmi160::ODR_HZ as f32,
                                    RATE_TOLERANCE_PERCENT
                                ),
                                "I{}: {:.1} frames/s ({} frames in {} ms; expected 400 +-{} %)",
                                k,
                                hz,
                                meter.frames(),
                                meter.span_us() / 1000,
                                RATE_TOLERANCE_PERCENT
                            );
                        }
                        phase = Phase::Knocks;
                        led2.set_high();
                        note!(
                            "phase 2: knock hard next to each module; press the user button to finish"
                        );
                    }
                    Phase::Knocks if button.is_low() => {
                        array.park(&mut i2c).await;
                        break 'run;
                    }
                    _ => {}
                }

                let mut seen = Peaks::new();
                let round = array
                    .drain_round(&mut i2c, |k, result| match result {
                        Ok(drain) => {
                            rates[k].burst(drain.at.as_micros(), drain.frames.len() as u32);
                            for sample in drain.frames {
                                let dynamic = baselines[k].track(sample, threshold_lsb);
                                seen.dynamic = seen.dynamic.max(dynamic);
                                seen.magnitude[k] = seen.magnitude[k].max(sample.magnitude());
                                for axis in sample.to_array() {
                                    seen.axis[k] = seen.axis[k].max(i32::from(axis).abs());
                                }
                            }
                        }
                        Err(_) => i2c_errors += 1,
                    })
                    .await;

                if phase == Phase::Knocks {
                    let now = Instant::now();
                    match &mut event {
                        Some((peaks, end)) => {
                            peaks.merge(&seen);
                            if now >= *end {
                                knocks += 1;
                                let mut line: String<160> = String::new();
                                let mut saturated = false;
                                for k in (0..SENSORS).filter(|k| array.is_ready(*k)) {
                                    // 9 x " I8 16.00" fits easily in 160 bytes.
                                    let _ = write!(
                                        line,
                                        " I{} {:.2}",
                                        k,
                                        peaks.magnitude[k] / LSB_PER_G
                                    );
                                    if peaks.magnitude[k] > worst.0 {
                                        worst = (peaks.magnitude[k], k, knocks);
                                    }
                                    if peaks.axis[k] >= SATURATION_LSB {
                                        saturated = true;
                                        defmt::warn!(
                                            "knock {=u32}: I{=usize} SATURATED: an axis reached {=f32} g",
                                            knocks,
                                            k,
                                            peaks.axis[k] as f32 / LSB_PER_G
                                        );
                                    }
                                }
                                if saturated {
                                    saturated_knocks += 1;
                                }
                                note!("knock {}: peak |a| in g:{}", knocks, line);
                                event = None;
                            }
                        }
                        None if now >= hold_until && seen.dynamic > threshold_lsb => {
                            event = Some((seen, now + EVENT_WINDOW));
                            hold_until = now + REFRACTORY;
                        }
                        None => {}
                    }
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
            fail!(
                report,
                "run abandoned after {} bus recoveries: the bus kept hanging",
                recoveries
            );
            break;
        }
    }
    led2.set_low();

    if phase == Phase::Still {
        fail!(report, "phase 1 did not finish");
    } else {
        check!(report, knocks > 0, "{} knocks detected", knocks);
        check!(
            report,
            saturated_knocks == 0,
            "{} of {} knocks saturated an axis (>= 15.5 g)",
            saturated_knocks,
            knocks
        );
        if knocks > 0 {
            note!(
                "worst peak: {:.2} g on I{} (knock {}); full scale is 16 g per axis",
                worst.0 / LSB_PER_G,
                worst.1,
                worst.2
            );
        }
    }
    if i2c_errors != 0 || recoveries != 0 {
        note!(
            "{} I2C errors, {} bus recoveries during the run",
            i2c_errors,
            recoveries
        );
    }

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
