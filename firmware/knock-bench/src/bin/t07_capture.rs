//! T7 / T8: knock capture (gain calibration, mounting comparison).
//!
//! Wiring: the full array on the plate (T7), or any subset: sensors that do
//! not answer are skipped. Without a multiplexer, a BMI160 at 0x68 directly on
//! I2C1 is used as I0 and one at 0x69 as I8 (the two-sensor T8 set-up).
//!
//! All FIFOs are drained every 20 ms into one ring buffer per sensor. A knock
//! is triggered when any sensor's dynamic acceleration |a − gravity| exceeds
//! `TRIGGER_MG` = **300 mg**. For every sensor the window from −10 ms to
//! +40 ms around the trigger is kept: 4 samples before and 16 from the trigger
//! on (sample numbers −4…+15 at 400 Hz). A new knock is accepted 300 ms after
//! the previous trigger at the earliest.
//!
//! Sample times are reconstructed (headerless FIFO frames carry none): the
//! newest frame of a burst gets the MCU time of the FIFO_LENGTH read and older
//! ones are spaced 2.5 ms apart. Sample 0 of each sensor is the sample nearest
//! to the trigger time, so the windows of different sensors are aligned to
//! about ±1 sample.
//!
//! Per knock the log shows the peak dynamic acceleration A_i of every sensor
//! in mg (baseline = mean of the four pre-trigger samples) and the sensor with
//! the largest peak. CSV on the virtual COM port (921600 8N1):
//!
//! ```text
//! # t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg     (once at start-up)
//! # t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg     (again before every knock)
//! 1,0,-4,-12,3,1001
//! ...                                  (20 rows per sensor, sensors in order)
//! # knock 1 peaks,<A0>,<A1>,...,<A8>
//! # t07_capture,knock,sensor,sample,x_mg,y_mg,z_mg
//! 2,0,-4,-11,3,1000
//! ...
//! # knock 2 peaks,<A0>,<A1>,...,<A8>
//! ...
//! # end
//! ```
//!
//! The header is repeated before every knock (as in the piezo tests), so a
//! capture script attached in mid-run still records the knocks that follow.
//! The repeats belong to the same run: one run, one file.
//!
//! Knocks are numbered from 1. `x_mg`…`z_mg` are the raw samples in mg
//! (integers, gravity included); the peaks are integers in mg. A sensor that
//! is absent (or whose window could not be extracted) has no sample rows and
//! the peak `nan`.
//!
//! **Knock as the test plan says; press the blue user button when done** (the
//! `# end` line is only sent then). LED2 toggles on every captured knock.
//!
//! Pass: at least one sensor configured, at least one knock captured, every
//! window complete, CSV written without error.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker};
use heapless::String;
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::knock::{
    self, Baseline, Earliest, Refractory, SAMPLE_PERIOD_US, SampleRing,
};
use knock_bench::drivers::{Accel, bmi160};
use knock_bench::imu::{self, ROUND_PERIOD, SENSORS};
use knock_bench::report::{self, CSV_LINE_CAP, CsvWriter, Report};
use knock_bench::{check, fail, note};

/// Dynamic acceleration that triggers a capture, in mg.
const TRIGGER_MG: f32 = 300.0;
/// Samples kept before the trigger sample (−10 ms).
const PRE: usize = 4;
/// Samples kept from the trigger sample on (0 … +37.5 ms).
const POST: usize = 16;
/// Window length in samples.
const WINDOW: usize = PRE + POST;
/// Hold-off after a trigger.
const REFRACTORY: Duration = Duration::from_millis(300);
/// If a window is still incomplete this long after the knock was detected,
/// the knock is written out with what is there.
const COLLECT_TIMEOUT: Duration = Duration::from_millis(200);
/// Ring buffer length per sensor (320 ms at 400 Hz).
const RING: usize = 128;
/// Give up after this many bus recoveries: the bus is not usable.
const MAX_RECOVERIES: u32 = 20;
/// Name and columns of the CSV stream.
const STREAM: &str = "t07_capture";
const COLUMNS: [&str; 6] = ["knock", "sensor", "sample", "x_mg", "y_mg", "z_mg"];

/// A triggered knock whose post-trigger samples are still arriving.
struct Pending {
    /// Sensor that crossed the threshold first.
    sensor: u8,
    /// Sample number of window sample 0, per sensor (`None`: sensor absent).
    centers: [Option<u64>; SENSORS],
    /// When to stop waiting for late samples.
    deadline: Instant,
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        vcp,
        mut led2,
        button,
        ..
    } = Board::init();
    let (tx, _rx) = vcp.split();
    let mut csv = CsvWriter::new(tx);

    let mut report = Report::begin("T07", "knock capture");
    note!(
        "I2C1 at {} Hz; trigger {} mg, window -{}..+{} samples ({} ms per sample), refractory {} ms",
        board::I2C1_HZ,
        TRIGGER_MG,
        PRE,
        POST - 1,
        SAMPLE_PERIOD_US as f32 / 1000.0,
        REFRACTORY.as_millis()
    );

    let mut array = imu::bring_up(&mut port).await;
    check!(
        report,
        array.ready_count() > 0,
        "{} of {} sensors configured (absent sensors are skipped)",
        array.ready_count(),
        SENSORS
    );
    if array.ready_count() == 0 {
        let verdict = report.finish();
        report::idle_blink(&mut led2, verdict).await
    }

    let threshold_lsb = TRIGGER_MG * bmi160::LSB_PER_G_16G as f32 / 1000.0;
    let mut rings = [const { SampleRing::<RING>::new() }; SENSORS];
    let mut baselines = [Baseline::new(); SENSORS];
    let mut refractory = Refractory::new();
    let mut pending: Option<Pending> = None;
    let mut knocks = 0u32;
    let mut incomplete = 0u32;
    let mut i2c_errors = 0u32;
    let mut recoveries = 0u32;
    let mut flushed = false;

    // Sent once here so an attached script opens its file at once, and again
    // before every knock for a script that attaches later.
    let mut csv_ok = csv.header(STREAM, &COLUMNS).await.is_ok();
    note!("ready: knock; press the user button to finish");

    'run: loop {
        {
            let mut i2c = TimeoutI2c::new(port.bus());
            if !flushed {
                if let Err((k, fault)) = array.flush_all(&mut i2c).await {
                    note!("I{}: FIFO flush failed: {:?}", k, fault);
                }
                flushed = true;
            }
            let mut ticker = Ticker::every(ROUND_PERIOD);
            loop {
                ticker.next().await;
                if button.is_low() {
                    array.park(&mut i2c).await;
                    break 'run;
                }

                // Drain every FIFO into its ring and look for the earliest
                // threshold crossing among the new samples.
                let mut earliest = Earliest::new();
                let round = array
                    .drain_round(&mut i2c, |k, result| match result {
                        Ok(drain) => {
                            let at_us = drain.at.as_micros();
                            let mut crossing = None;
                            let mut any = false;
                            for sample in drain.frames {
                                any = true;
                                let seq = rings[k].push(sample);
                                if baselines[k].track(sample, threshold_lsb) > threshold_lsb
                                    && crossing.is_none()
                                {
                                    crossing = Some(seq);
                                }
                            }
                            if any {
                                rings[k].stamp(at_us);
                            }
                            if let Some(seq) = crossing
                                && let Some(t_us) = rings[k].time_of(seq)
                            {
                                earliest.offer(t_us, k as u8);
                            }
                        }
                        Err(_) => i2c_errors += 1,
                    })
                    .await;

                match &pending {
                    None => {
                        if let Some((t_us, sensor)) = earliest.get()
                            && refractory.ready(t_us)
                        {
                            let mut centers = [None; SENSORS];
                            for k in (0..SENSORS).filter(|k| array.is_ready(*k)) {
                                centers[k] = rings[k].seq_at(t_us);
                            }
                            refractory.start(t_us, REFRACTORY.as_micros());
                            pending = Some(Pending {
                                sensor,
                                centers,
                                deadline: Instant::now() + COLLECT_TIMEOUT,
                            });
                        }
                    }
                    Some(p) => {
                        let complete = (0..SENSORS).all(|k| {
                            p.centers[k].is_none_or(|c| knock::window_complete(&rings[k], c, POST))
                        });
                        if complete || Instant::now() >= p.deadline {
                            knocks += 1;
                            led2.toggle();

                            // Cut the windows and compute the peaks.
                            let mut windows = [[Accel::new(0, 0, 0); WINDOW]; SENSORS];
                            let mut have = [false; SENSORS];
                            let mut peaks_mg = [0.0f32; SENSORS];
                            for k in 0..SENSORS {
                                let Some(center) = p.centers[k] else { continue };
                                match knock::extract_window(&rings[k], center, PRE, &mut windows[k])
                                {
                                    Ok(()) => {
                                        have[k] = true;
                                        peaks_mg[k] = bmi160::raw_to_mg_16g_f32(
                                            knock::peak_dynamic(&windows[k], PRE),
                                        );
                                    }
                                    Err(e) => {
                                        incomplete += 1;
                                        defmt::warn!(
                                            "knock {=u32}: I{=usize}: window not available: {}",
                                            knocks,
                                            k,
                                            e
                                        );
                                    }
                                }
                            }

                            let mut line: String<160> = String::new();
                            for k in (0..SENSORS).filter(|k| have[*k]) {
                                // 9 x " I8 32000" fits easily in 160 bytes.
                                let _ = write!(line, " I{} {:.0}", k, peaks_mg[k]);
                            }
                            match knock::argmax(&peaks_mg) {
                                Some(best) => note!(
                                    "knock {}: triggered by I{}; peak A in mg:{}; largest: I{}",
                                    knocks,
                                    p.sensor,
                                    line,
                                    best
                                ),
                                None => note!(
                                    "knock {}: triggered by I{}; no window available",
                                    knocks,
                                    p.sensor
                                ),
                            }

                            // CSV: the header again, 20 rows per sensor, then
                            // the peaks line.
                            if csv_ok {
                                csv_ok = csv.header(STREAM, &COLUMNS).await.is_ok();
                            }
                            for k in (0..SENSORS).filter(|k| have[*k]) {
                                for (i, s) in windows[k].iter().enumerate() {
                                    if !csv_ok {
                                        break;
                                    }
                                    csv_ok = csv
                                        .row(format_args!(
                                            "{},{},{},{},{},{}",
                                            knocks,
                                            k,
                                            i as i32 - PRE as i32,
                                            bmi160::raw_to_mg_16g(s.x),
                                            bmi160::raw_to_mg_16g(s.y),
                                            bmi160::raw_to_mg_16g(s.z)
                                        ))
                                        .await
                                        .is_ok();
                                }
                            }
                            if csv_ok {
                                // `nan` marks a sensor without a window, so a
                                // real reading of 0 mg stays distinguishable.
                                let mut text: String<CSV_LINE_CAP> = String::new();
                                let mut fits = write!(text, "knock {knocks} peaks").is_ok();
                                for k in 0..SENSORS {
                                    fits &= if have[k] {
                                        write!(text, ",{:.0}", peaks_mg[k]).is_ok()
                                    } else {
                                        text.push_str(",nan").is_ok()
                                    };
                                }
                                csv_ok = fits && csv.comment(format_args!("{text}")).await.is_ok();
                            }
                            pending = None;
                        }
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

    if csv_ok {
        csv_ok = csv.end().await.is_ok();
    }

    check!(report, knocks > 0, "{} knocks captured", knocks);
    check!(
        report,
        incomplete == 0,
        "{} sensor windows could not be extracted",
        incomplete
    );
    check!(
        report,
        csv_ok,
        "CSV written to the virtual COM port without error"
    );
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
