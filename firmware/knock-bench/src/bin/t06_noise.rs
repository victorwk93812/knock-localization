//! T6: noise floor of every accelerometer.
//!
//! Wiring: the full array mounted on the plate, plate on its feet on the
//! table. The onboard LSM6DSL (I2C2, 0x6A) is recorded as the table reference.
//!
//! **Leave plate and table completely alone for 60 s.** All FIFOs are drained
//! every 20 ms, so every 400 Hz sample of the nine BMI160s is used (about
//! 24 000 per sensor); the LSM6DSL is read once per round (50 samples/s of its
//! 416 Hz output, about 3000 samples).
//!
//! For each sensor the log shows, in mg: the mean and the RMS noise (standard
//! deviation) of each axis, the mean magnitude |a| (≈ 1000 at rest), the RMS
//! of the dynamic magnitude |a − mean| and five times that as the suggested
//! trigger threshold. A sensor whose dynamic RMS is more than 3× the median of
//! the BMI160s is pointed out (loose mount, cable touching the plate?).
//!
//! CSV on the virtual COM port (921600 8N1), one row per sensor and axis:
//!
//! ```text
//! # t06_noise,sensor,axis,mean_mg,rms_mg
//! 0,x,-12.41,1.83
//! 0,y,3.05,1.79
//! 0,z,1001.22,2.60
//! 0,mag,1001.31,3.64
//! ...
//! # end
//! ```
//!
//! `sensor` is 0–8 for I0..I8 and 9 for the table reference; `axis` is `x`,
//! `y`, `z` or `mag` (mean of |a|, RMS of the dynamic magnitude). Sensors that
//! delivered no samples have no rows.
//!
//! Pass: all nine BMI160s and the reference delivered samples.
//!
//! Build with `--features knock-bench/quick` for a 10 s run. Ends with the
//! RESULT line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Ticker, Timer};
use embedded_io_async::Write;
use heapless::Vec;
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::lsm6dsl::{self, Lsm6dsl};
use knock_bench::drivers::running::{self, AccelAccum};
use knock_bench::drivers::{bmi160, health::ErrorCounters};
use knock_bench::imu::{self, ROUND_PERIOD, SENSORS};
use knock_bench::report::{self, CsvError, CsvWriter, Report};
use knock_bench::{check, fail, note, row};

/// Length of the recording.
const DURATION: Duration = if cfg!(feature = "quick") {
    Duration::from_secs(10)
} else {
    Duration::from_secs(60)
};
/// Suggested trigger threshold as a multiple of the noise RMS.
const THRESHOLD_SIGMAS: f32 = 5.0;
/// A sensor noisier than this multiple of the median is pointed out.
const NOISY_FACTOR: f32 = 3.0;
/// Sensor number of the table reference in the CSV.
const REFERENCE_ID: usize = 9;
/// Give up after this many bus recoveries: the bus is not usable.
const MAX_RECOVERIES: u32 = 20;

const REFERENCE: Lsm6dsl = Lsm6dsl::new(lsm6dsl::DEFAULT_ADDRESS);

/// Print one sensor's table row and CSV rows. Returns the RMS of the dynamic
/// magnitude in mg, or `None` if the sensor has no samples.
async fn emit<W: Write>(
    csv: &mut CsvWriter<W>,
    id: usize,
    acc: &AccelAccum,
    to_mg: fn(f32) -> f32,
) -> Result<Option<f32>, CsvError<W::Error>> {
    let (Some(mean_mag), Some(dynamic)) = (acc.mean_magnitude(), acc.dynamic_rms()) else {
        row!("  {}    | no samples", id);
        return Ok(None);
    };
    let mut mean = [0.0f32; 3];
    let mut rms = [0.0f32; 3];
    for (i, axis) in acc.axes().iter().enumerate() {
        mean[i] = to_mg(axis.mean().unwrap_or(0.0));
        rms[i] = to_mg(axis.std_dev().unwrap_or(0.0));
    }
    let (mean_mag, dynamic) = (to_mg(mean_mag), to_mg(dynamic));
    row!(
        "  {}    | {:>6} | {:>7.1} {:>7.1} {:>7.1} | {:>5.2} {:>5.2} {:>5.2} | {:>6.1} | {:>7.2} | {:>7.1}",
        id,
        acc.count(),
        mean[0],
        mean[1],
        mean[2],
        rms[0],
        rms[1],
        rms[2],
        mean_mag,
        dynamic,
        THRESHOLD_SIGMAS * dynamic
    );
    for (name, m, r) in [
        ("x", mean[0], rms[0]),
        ("y", mean[1], rms[1]),
        ("z", mean[2], rms[2]),
        ("mag", mean_mag, dynamic),
    ] {
        csv.row(format_args!("{id},{name},{m:.2},{r:.2}")).await?;
    }
    Ok(Some(dynamic))
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        i2c2,
        vcp,
        mut led2,
        ..
    } = Board::init();
    let (tx, _rx) = vcp.split();
    let mut csv = CsvWriter::new(tx);

    let mut report = Report::begin("T06", "noise floor");
    note!(
        "I2C1 at {} Hz; recording {} s: leave the plate and the table alone",
        board::I2C1_HZ,
        DURATION.as_secs()
    );

    // Table reference on I2C2.
    let mut ref_bus = TimeoutI2c::new(i2c2);
    let ref_ready = match REFERENCE.who_am_i(&mut ref_bus).await {
        Ok(lsm6dsl::WHO_AM_I_VALUE) => match REFERENCE.init_accel(&mut ref_bus).await {
            Ok(()) => true,
            Err(e) => {
                note!("table reference: configuration failed: {:?}", e);
                false
            }
        },
        Ok(other) => {
            note!("table reference: WHO_AM_I = {:#04x}, expected 0x6a", other);
            false
        }
        Err(e) => {
            note!("table reference: no answer at 0x6a on I2C2: {:?}", e);
            false
        }
    };

    let mut array = imu::bring_up(&mut port).await;
    // Let the accelerometers' filters and the LSM6DSL settle.
    Timer::after(Duration::from_millis(100)).await;

    let mut accums = [AccelAccum::new(); SENSORS];
    let mut ref_accum = AccelAccum::new();
    let mut errors = [ErrorCounters::new(); SENSORS];
    let mut ref_errors = 0u32;
    let mut recoveries = 0u32;
    let mut flushed = false;
    let mut started = Instant::now();

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
            let mut rounds = 0u32;
            loop {
                ticker.next().await;
                if started.elapsed() >= DURATION {
                    array.park(&mut i2c).await;
                    break 'run;
                }
                rounds += 1;
                if rounds.is_multiple_of(25) {
                    led2.toggle();
                }

                let round = array
                    .drain_round(&mut i2c, |k, result| match result {
                        Ok(drain) => {
                            errors[k].record_ok();
                            for sample in drain.frames {
                                accums[k].push(sample);
                            }
                        }
                        Err(fault) => errors[k].record(fault.class()),
                    })
                    .await;

                if ref_ready {
                    match REFERENCE.read_accel(&mut ref_bus).await {
                        Ok(sample) => ref_accum.push(sample),
                        Err(_) => ref_errors += 1,
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

    // Results: table over defmt, summary rows as CSV.
    let mut csv_result = csv
        .header("t06_noise", &["sensor", "axis", "mean_mg", "rms_mg"])
        .await;
    row!(
        "sensor | samples |   mean x       y       z   | rms x     y     z |  |a|   | dyn rms | 5 sigma   (all in mg; 9 = table reference)"
    );
    let mut dynamic_mg: [Option<f32>; SENSORS] = [None; SENSORS];
    for k in 0..SENSORS {
        if csv_result.is_ok() {
            match emit(&mut csv, k, &accums[k], bmi160::raw_to_mg_16g_f32).await {
                Ok(d) => dynamic_mg[k] = d,
                Err(e) => csv_result = Err(e),
            }
        }
    }
    let mut ref_dynamic = None;
    if csv_result.is_ok() {
        match emit(
            &mut csv,
            REFERENCE_ID,
            &ref_accum,
            lsm6dsl::raw_to_mg_16g_f32,
        )
        .await
        {
            Ok(d) => ref_dynamic = d,
            Err(e) => csv_result = Err(e),
        }
    }
    if csv_result.is_ok() {
        csv_result = csv.end().await;
    }
    if let Err(e) = csv_result {
        fail!(report, "CSV output on the virtual COM port failed: {:?}", e);
    }

    for (k, accum) in accums.iter().enumerate() {
        check!(
            report,
            accum.count() > 0,
            "I{}: {} samples{}",
            k,
            accum.count(),
            match array.fault(k) {
                Some(_) => " (not configured, see bring-up above)",
                None => "",
            }
        );
    }
    check!(
        report,
        ref_accum.count() > 0,
        "table reference (LSM6DSL): {} samples",
        ref_accum.count()
    );

    let mut values: Vec<f32, SENSORS> = dynamic_mg.iter().flatten().copied().collect();
    if let Some(median) = running::median(&mut values) {
        let loudest = values.last().copied().unwrap_or(median);
        note!(
            "BMI160 dynamic RMS: median {:.2} mg, largest {:.2} mg; suggested trigger threshold (5 sigma of the noisiest) {:.1} mg",
            median,
            loudest,
            THRESHOLD_SIGMAS * loudest
        );
        for (k, d) in dynamic_mg.iter().enumerate() {
            if let Some(d) = d
                && *d > NOISY_FACTOR * median
            {
                defmt::warn!(
                    "FLAG: I{=usize} is noisy: {=f32} mg dynamic RMS is more than 3x the median ({=f32} mg)",
                    k,
                    *d,
                    median
                );
            }
        }
    }
    if let Some(d) = ref_dynamic {
        note!(
            "table reference dynamic RMS: {:.2} mg (different sensor type, not part of the median)",
            d
        );
    }
    let i2c_errors: u32 = errors.iter().map(|e| e.errors()).sum();
    if i2c_errors != 0 || ref_errors != 0 || recoveries != 0 {
        note!(
            "{} I2C1 errors, {} reference read errors, {} bus recoveries during the run",
            i2c_errors,
            ref_errors,
            recoveries
        );
    }

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
