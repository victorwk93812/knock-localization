//! T1: incoming check of the GY-BMI160 modules, one at a time.
//!
//! Wiring: **one** module directly on I2C1 (SDA = D14, SCL = D15, 3.3 V, GND),
//! no multiplexer. For each module:
//!
//! 1. scan the bus: exactly one of 0x68 / 0x69 must answer, which also tells
//!    how the SAO strap is set (0x68 = low = I0..I7, 0x69 = high = I8);
//! 2. CHIP_ID must read 0xD1;
//! 3. configure it (accel normal, 400 Hz, ±16 g, headerless FIFO);
//! 4. collect about 1 s of samples through the FIFO and report the mean per
//!    axis in mg, the gravity magnitude (must be 0.9–1.1 g with the module at
//!    rest) and the RMS noise per axis.
//!
//! Then swap in the next module and press the blue user button. While waiting,
//! LED2 is steady on if the last module passed and blinks fast if it failed.
//! Up to 10 modules (numbered 0–9) are checked; hold the button for 2 s to
//! finish early. The run ends with the RESULT line, then LED2 blinks slowly on
//! PASS and fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::Output;
use embassy_stm32::mode::Async;
use embassy_time::{Delay, Duration, Instant, Timer, with_timeout};
use embedded_hal_async::i2c::I2c;
use heapless::Vec;
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::bmi160::{self, Bmi160};
use knock_bench::drivers::scan::AddressSet;
use knock_bench::drivers::{fifo, stats};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, pass, scan};

/// Number of modules to check (9 sensors + 1 spare), numbered from 0.
const MODULES: u8 = 10;
/// Length of the sampling window per module.
const WINDOW: Duration = Duration::from_secs(1);
/// FIFO polling interval (20 frames = 120 bytes at 400 Hz).
const POLL: Duration = Duration::from_millis(50);
/// Capacity of the per-axis sample buffers.
const MAX_SAMPLES: usize = 512;
/// Accepted number of samples in the window: 400 Hz × 1 s ± 10 %.
const SAMPLE_COUNT: core::ops::RangeInclusive<usize> = 360..=440;
/// Accepted gravity magnitude with the module at rest, in mg.
const GRAVITY_MG: core::ops::RangeInclusive<f32> = 900.0..=1100.0;
/// Holding the button this long ends the run early.
const LONG_PRESS: Duration = Duration::from_secs(2);

/// Check one module. `Err` means the bus itself failed (the caller records
/// the failure and runs bus recovery); every other problem is recorded in
/// `report` and returns `Ok`.
async fn check_module<I: I2c>(i2c: &mut I, report: &mut Report, n: u8) -> Result<(), I::Error> {
    let found = scan::scan(i2c).await?;
    let addr = match (
        found.contains(bmi160::ADDR_SDO_LOW),
        found.contains(bmi160::ADDR_SDO_HIGH),
    ) {
        (false, false) => {
            fail!(
                report,
                "module {}: nothing at 0x68 or 0x69 (scan found {}): check CS -> 3.3 V strap, power, SDA/SCL swap",
                n,
                found
            );
            return Ok(());
        }
        (true, true) => {
            fail!(
                report,
                "module {}: both 0x68 and 0x69 answer: wire exactly one module for this test",
                n
            );
            return Ok(());
        }
        (true, false) => {
            pass!(
                report,
                "module {}: found at 0x68 (SAO strap low: fits I0..I7 behind the mux)",
                n
            );
            bmi160::ADDR_SDO_LOW
        }
        (false, true) => {
            pass!(
                report,
                "module {}: found at 0x69 (SAO strap high: fits I8 on the trunk)",
                n
            );
            bmi160::ADDR_SDO_HIGH
        }
    };
    let others = found.difference(&AddressSet::from_slice(&[
        bmi160::ADDR_SDO_LOW,
        bmi160::ADDR_SDO_HIGH,
    ]));
    if !others.is_empty() {
        note!("module {}: other devices on the bus: {}", n, others);
    }

    let imu = Bmi160::new(addr);
    let id = imu.chip_id(i2c).await?;
    if !check!(
        report,
        id == bmi160::CHIP_ID_VALUE,
        "module {}: CHIP_ID = {:#04x} (expected 0xd1)",
        n,
        id
    ) {
        note!(
            "module {}: wrong CHIP_ID: this is not a BMI160 (mislabelled or counterfeit part?)",
            n
        );
        return Ok(());
    }

    match imu.init_accel_fifo(i2c, &mut Delay).await {
        Ok(()) => pass!(
            report,
            "module {}: configured (accel normal, 400 Hz, +-16 g, headerless FIFO)",
            n
        ),
        Err(bmi160::Error::Bus(e)) => return Err(e),
        Err(e) => {
            fail!(report, "module {}: configuration failed: {:?}", n, e);
            return Ok(());
        }
    }

    // Collect about one second of samples through the FIFO.
    let mut xs: Vec<i16, MAX_SAMPLES> = Vec::new();
    let mut ys: Vec<i16, MAX_SAMPLES> = Vec::new();
    let mut zs: Vec<i16, MAX_SAMPLES> = Vec::new();
    let mut buf = [0u8; 42 * fifo::FRAME_LEN];
    let start = Instant::now();
    while start.elapsed() < WINDOW {
        Timer::after(POLL).await;
        let len = imu.read_fifo_frames(i2c, &mut buf).await?;
        for sample in fifo::frames(&buf[..len]) {
            if xs.push(sample.x).is_err() {
                break;
            }
            // Same capacity as `xs`, so these cannot fail once that push succeeded.
            let _ = ys.push(sample.y);
            let _ = zs.push(sample.z);
        }
    }

    check!(
        report,
        SAMPLE_COUNT.contains(&xs.len()),
        "module {}: {} samples in {} ms (expected {}..{} at 400 Hz)",
        n,
        xs.len(),
        start.elapsed().as_millis(),
        SAMPLE_COUNT.start(),
        SAMPLE_COUNT.end()
    );

    let axis_mg = |v: &[i16]| {
        (
            bmi160::raw_to_mg_16g_f32(stats::mean(v).unwrap_or(0.0)),
            bmi160::raw_to_mg_16g_f32(stats::std_dev(v).unwrap_or(0.0)),
        )
    };
    let (mx, nx) = axis_mg(&xs);
    let (my, ny) = axis_mg(&ys);
    let (mz, nz) = axis_mg(&zs);
    let gravity = stats::magnitude(mx, my, mz);
    note!(
        "module {}: mean x {:.0}, y {:.0}, z {:.0} mg",
        n,
        mx,
        my,
        mz
    );
    note!(
        "module {}: RMS noise x {:.1}, y {:.1}, z {:.1} mg",
        n,
        nx,
        ny,
        nz
    );
    check!(
        report,
        GRAVITY_MG.contains(&gravity),
        "module {}: |a| = {:.0} mg (expected {:.0}..{:.0} at rest)",
        n,
        gravity,
        GRAVITY_MG.start(),
        GRAVITY_MG.end()
    );

    let err = imu.error_status(i2c).await?;
    check!(
        report,
        err.is_ok(),
        "module {}: ERR_REG = {:#04x} after sampling",
        n,
        err.0
    );
    Ok(())
}

/// What the user asked for with the button.
enum Press {
    /// Short press: check the next module.
    Next,
    /// Held for [`LONG_PRESS`]: finish the run.
    Finish,
}

/// Show the last module's outcome on LED2 until the button is pressed.
async fn wait_for_user(
    button: &mut ExtiInput<'static, Async>,
    led: &mut Output<'static>,
    last_ok: bool,
) -> Press {
    let indicate = async {
        if last_ok {
            led.set_high();
            core::future::pending::<()>().await
        } else {
            loop {
                led.toggle();
                Timer::after(report::BLINK_FAIL).await;
            }
        }
    };
    select(board::wait_for_press(button), indicate).await;
    led.set_low();

    // The button is down now: a release within LONG_PRESS is a short press.
    match with_timeout(LONG_PRESS, button.wait_for_high()).await {
        Ok(()) => Press::Next,
        Err(_) => Press::Finish,
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        mut button,
        ..
    } = Board::init();

    let mut report = Report::begin("T01", "chip id");
    note!(
        "I2C1 at {} Hz; wire ONE module to D14 (SDA) / D15 (SCL), no mux",
        board::I2C1_HZ
    );

    let mut checked = 0u8;
    let mut good = 0u8;
    for n in 0..MODULES {
        defmt::info!("--- module {=u8} ---", n);
        let failed_before = report.failed();

        let outcome = {
            let mut i2c = TimeoutI2c::new(port.bus());
            check_module(&mut i2c, &mut report, n).await
        };
        if let Err(e) = outcome {
            fail!(
                report,
                "module {}: I2C error {:?}: bus stuck or module unplugged during the test",
                n,
                e
            );
            let state = port.recover().await;
            note!("bus recovery: {:?}", state);
        }

        let ok = report.failed() == failed_before;
        checked += 1;
        if ok {
            good += 1;
            defmt::info!("module {=u8}: OK", n);
        } else {
            defmt::error!("module {=u8}: FAILED", n);
        }

        if n + 1 == MODULES {
            break;
        }
        defmt::info!(
            "swap in module {=u8} (unplug VCC first), then press the user button; hold 2 s to finish",
            n + 1
        );
        if let Press::Finish = wait_for_user(&mut button, &mut led2, ok).await {
            note!("finished early by long press");
            break;
        }
    }

    note!("{} of {} checked modules passed", good, checked);
    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
