//! T00: board bring-up, with nothing but the Discovery kit required.
//!
//! - LED2 blinks three times (visual check).
//! - The virtual COM port (USART1, 921600 8N1) prints a reset banner, then a
//!   background task echoes every received byte.
//! - The onboard LSM6DSL on I2C2 answers WHO_AM_I = 0x6A and reports a gravity
//!   vector of 0.8–1.2 g.
//! - I2C1 (Arduino D14/D15) is scanned and the result listed; this part is
//!   informational, because nothing needs to be wired for T00.
//!
//! Ends with the RESULT line, then LED2 blinks slowly on PASS, fast on FAIL.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_hal_async::i2c::I2c;
use knock_bench::board::{self, Board, Vcp};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::lsm6dsl::{self, Lsm6dsl};
use knock_bench::drivers::{Accel, stats};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, pass, scan};

const VCP_BANNER: &[u8] =
    b"\r\n=== T00 board === knock-bench reset; typed bytes are echoed back\r\n";

/// Accepted gravity magnitude from the table-reference accelerometer, in mg.
const GRAVITY_MG: core::ops::RangeInclusive<i32> = 800..=1200;

/// Echo every byte received on the virtual COM port.
#[embassy_executor::task]
async fn vcp_echo(mut vcp: Vcp) {
    let mut buf = [0u8; 64];
    loop {
        match vcp.read_until_idle(&mut buf).await {
            Ok(0) => {}
            Ok(n) => {
                if let Err(e) = vcp.write(&buf[..n]).await {
                    defmt::warn!("VCP echo write error: {}", e);
                }
            }
            Err(e) => defmt::warn!("VCP receive error: {}", e),
        }
    }
}

/// Configure the LSM6DSL and read one settled accelerometer sample.
async fn table_sample<I: I2c>(i2c: &mut I, imu: Lsm6dsl) -> Result<Accel, I::Error> {
    imu.init_accel(i2c).await?;
    // Let the output settle after power-up (a few tens of samples at 416 Hz).
    Timer::after(Duration::from_millis(100)).await;
    imu.read_accel(i2c).await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let Board {
        mut i2c1,
        i2c2,
        mut vcp,
        mut led2,
        button: _,
        piezo: _,
    } = Board::init();

    let mut report = Report::begin("T00", "board");
    note!(
        "SYSCLK {} Hz, I2C1 {} Hz, I2C2 {} Hz, VCP {} baud",
        board::SYSCLK_HZ,
        board::I2C1_HZ,
        board::I2C2_HZ,
        board::VCP_BAUD
    );

    // LED2: three quick blinks.
    for _ in 0..3 {
        led2.set_high();
        Timer::after(Duration::from_millis(100)).await;
        led2.set_low();
        Timer::after(Duration::from_millis(100)).await;
    }
    note!("LED2 blinked 3 times (check by eye)");

    // Virtual COM port: banner, then echo in the background.
    match vcp.write(VCP_BANNER).await {
        Ok(()) => pass!(
            report,
            "VCP banner sent ({} baud 8N1); check it on the host, typed bytes are echoed",
            board::VCP_BAUD
        ),
        Err(e) => fail!(report, "VCP banner write failed: {:?}", e),
    }
    match vcp_echo(vcp) {
        Ok(token) => spawner.spawn(token),
        Err(e) => fail!(report, "could not start the VCP echo task: {:?}", e),
    }

    // Onboard LSM6DSL on I2C2.
    let mut bus2 = TimeoutI2c::new(i2c2);
    let table = Lsm6dsl::new(lsm6dsl::DEFAULT_ADDRESS);
    match table.who_am_i(&mut bus2).await {
        Ok(id) => {
            check!(
                report,
                id == lsm6dsl::WHO_AM_I_VALUE,
                "LSM6DSL WHO_AM_I = {:#04x} (expected {:#04x})",
                id,
                lsm6dsl::WHO_AM_I_VALUE
            );
        }
        Err(e) => fail!(report, "LSM6DSL WHO_AM_I read failed: {:?}", e),
    }
    match table_sample(&mut bus2, table).await {
        Ok(a) => {
            let x = lsm6dsl::raw_to_mg_16g(a.x);
            let y = lsm6dsl::raw_to_mg_16g(a.y);
            let z = lsm6dsl::raw_to_mg_16g(a.z);
            let mag = stats::magnitude(x as f32, y as f32, z as f32) as i32;
            check!(
                report,
                GRAVITY_MG.contains(&mag),
                "LSM6DSL |a| = {} mg (x {}, y {}, z {}), expected {}..{} with the board at rest",
                mag,
                x,
                y,
                z,
                GRAVITY_MG.start(),
                GRAVITY_MG.end()
            );
        }
        Err(e) => fail!(report, "LSM6DSL accelerometer read failed: {:?}", e),
    }

    // I2C1 scan: informational only.
    {
        let mut bus1 = TimeoutI2c::new(i2c1.bus());
        match scan::scan(&mut bus1).await {
            Ok(found) if found.is_empty() => {
                note!("I2C1 scan: nothing answered (expected with nothing wired to D14/D15)");
            }
            Ok(found) => note!("I2C1 scan: {} device(s) at {}", found.len(), found),
            Err(e) => note!(
                "I2C1 scan aborted: {:?} (bus stuck? check wiring and pull-ups)",
                e
            ),
        }
    }

    // TODO(piezo): read the idle level of A0..A3 here (ADC1 and the pins are in
    // `Board::piezo`); expected about mid-scale once the front end is wired.

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
