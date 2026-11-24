//! T2: TCA9548A multiplexer and the trunk sensor I8.
//!
//! Wiring: the multiplexer (0x70) on I2C1, BMI160 I0..I7 (0x68) on its
//! channels 0..7, BMI160 I8 (0x69) directly on the I2C1 trunk.
//!
//! 1. The multiplexer must answer at 0x70.
//! 2. For each channel k: write `1 << k`, read it back, scan the bus. 0x68
//!    (the channel's sensor), 0x69 (I8) and 0x70 (the mux) must be visible,
//!    and CHIP_ID read through the channel must be 0xD1.
//! 3. With every channel disabled, 0x68 must not answer.
//! 4. CHIP_ID of I8 at 0x69 must be 0xD1.
//!
//! One table row is printed per channel; the verdict names the failing
//! channels. The multiplexer is left with all channels disabled. Ends with the
//! RESULT line, then LED2 blinks slowly on PASS and fast on FAIL.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use embassy_executor::Spawner;
use embedded_hal_async::i2c::I2c;
use heapless::String;
use knock_bench::board::{self, Board};
use knock_bench::bus::TimeoutI2c;
use knock_bench::drivers::bmi160::{self, Bmi160};
use knock_bench::drivers::scan::AddressSet;
use knock_bench::drivers::tca9548a::{self, Tca9548a};
use knock_bench::report::{self, Report};
use knock_bench::{check, fail, note, row, scan};

const MUX: Tca9548a = Tca9548a::new(tca9548a::DEFAULT_ADDRESS);
const CHANNEL_IMU: Bmi160 = Bmi160::new(bmi160::ADDR_SDO_LOW);
const TRUNK_IMU: Bmi160 = Bmi160::new(bmi160::ADDR_SDO_HIGH);

/// The only addresses expected anywhere on I2C1.
const EXPECTED: AddressSet = AddressSet::from_slice(&[
    bmi160::ADDR_SDO_LOW,
    bmi160::ADDR_SDO_HIGH,
    tca9548a::DEFAULT_ADDRESS,
]);

/// What was observed with one multiplexer channel selected.
struct ChannelRow {
    readback: u8,
    found: AddressSet,
    chip_id: Option<u8>,
}

/// Select channel `k`, read the control register back, scan, and read CHIP_ID
/// through the channel if a device answers at 0x68.
async fn probe_channel<I: I2c>(i2c: &mut I, k: u8) -> Result<ChannelRow, I::Error> {
    MUX.select(i2c, 1 << k).await?;
    let readback = MUX.readback(i2c).await?;
    let found = scan::scan(i2c).await?;
    let chip_id = if found.contains(CHANNEL_IMU.address()) {
        Some(CHANNEL_IMU.chip_id(i2c).await?)
    } else {
        None
    };
    Ok(ChannelRow {
        readback,
        found,
        chip_id,
    })
}

/// Disable every channel, read the control register back, scan, and read
/// CHIP_ID of I8 if a device answers at 0x69.
///
/// Returns (control register, addresses found, CHIP_ID of I8).
async fn probe_trunk<I: I2c>(i2c: &mut I) -> Result<(u8, AddressSet, Option<u8>), I::Error> {
    MUX.disable_all(i2c).await?;
    let readback = MUX.readback(i2c).await?;
    let found = scan::scan(i2c).await?;
    let trunk_id = if found.contains(TRUNK_IMU.address()) {
        Some(TRUNK_IMU.chip_id(i2c).await?)
    } else {
        None
    };
    Ok((readback, found, trunk_id))
}

/// "yes" / "NO" for a table cell.
fn seen(set: &AddressSet, addr: u8) -> &'static str {
    if set.contains(addr) { "yes" } else { "NO" }
}

/// Append the channel numbers in `mask` to `out` as "0, 3, 7".
fn list_channels(mask: u8, out: &mut String<32>) {
    for k in (0..tca9548a::CHANNELS).filter(|k| mask & (1 << k) != 0) {
        // 32 bytes hold all eight channels ("0, 1, 2, 3, 4, 5, 6, 7").
        let _ = if out.is_empty() {
            write!(out, "{k}")
        } else {
            write!(out, ", {k}")
        };
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let Board {
        i2c1: mut port,
        mut led2,
        ..
    } = Board::init();

    let mut report = Report::begin("T02", "mux scan");
    note!("I2C1 at {} Hz", board::I2C1_HZ);

    // 1. The multiplexer itself.
    let mux_present = {
        let mut i2c = TimeoutI2c::new(port.bus());
        scan::probe(&mut i2c, MUX.address()).await
    };
    let mux_ok = match mux_present {
        Ok(present) => check!(report, present, "mux answers at {:#04x}", MUX.address()),
        Err(e) => {
            fail!(report, "mux probe: I2C error {:?}", e);
            let state = port.recover().await;
            note!("bus recovery: {:?}", state);
            false
        }
    };
    if !mux_ok {
        note!(
            "no multiplexer: check its VCC/GND, SDA -> D14, SCL -> D15, and that A0..A2 are low (address 0x70)"
        );
        let verdict = report.finish();
        report::idle_blink(&mut led2, verdict).await
    }

    // 2. Every channel in turn.
    let mut failing_channels = 0u8; // bit k: channel k failed
    let mut trunk_missing = 0u8; // bit k: 0x69 or 0x70 not visible with channel k selected
    row!("ch | readback | 0x68 | 0x69 | 0x70 | CHIP_ID | result");
    for k in 0..tca9548a::CHANNELS {
        let outcome = {
            let mut i2c = TimeoutI2c::new(port.bus());
            probe_channel(&mut i2c, k).await
        };
        let observed = match outcome {
            Ok(observed) => observed,
            Err(e) => {
                failing_channels |= 1 << k;
                row!(" {} | I2C error {:?}", k, e);
                fail!(
                    report,
                    "ch{}: I2C error {:?}: wiring fault on this channel?",
                    k,
                    e
                );
                let state = port.recover().await;
                note!("bus recovery: {:?}", state);
                continue;
            }
        };

        let readback_ok = observed.readback == 1 << k;
        let sensor_ok = observed.chip_id == Some(bmi160::CHIP_ID_VALUE);
        let trunk_ok =
            observed.found.contains(TRUNK_IMU.address()) && observed.found.contains(MUX.address());
        let channel_ok = readback_ok && sensor_ok;
        if !channel_ok {
            failing_channels |= 1 << k;
        }
        if !trunk_ok {
            trunk_missing |= 1 << k;
        }

        let mut id: String<8> = String::new();
        // "0xd1" or "-" always fits in 8 bytes.
        let _ = match observed.chip_id {
            Some(v) => write!(id, "{v:#04x}"),
            None => write!(id, "-"),
        };
        row!(
            " {} |   {:#04x}   | {:>4} | {:>4} | {:>4} | {:>7} | {}",
            k,
            observed.readback,
            seen(&observed.found, CHANNEL_IMU.address()),
            seen(&observed.found, TRUNK_IMU.address()),
            seen(&observed.found, MUX.address()),
            id,
            if channel_ok && trunk_ok { "ok" } else { "FAIL" }
        );

        check!(
            report,
            readback_ok,
            "ch{}: control register reads back {:#04x} (expected {:#04x})",
            k,
            observed.readback,
            1u8 << k
        );
        match observed.chip_id {
            Some(v) => check!(
                report,
                sensor_ok,
                "ch{}: BMI160 at 0x68, CHIP_ID = {:#04x} (expected 0xd1)",
                k,
                v
            ),
            None => check!(
                report,
                false,
                "ch{}: nothing at 0x68: check this channel's SDA/SCL/power and the module's CS and SAO straps",
                k
            ),
        };
        check!(
            report,
            trunk_ok,
            "ch{}: 0x69 (I8) and 0x70 (mux) visible on the trunk",
            k
        );
        let unexpected = observed.found.difference(&EXPECTED);
        if !unexpected.is_empty() {
            note!("ch{}: unexpected devices: {}", k, unexpected);
        }
    }

    // 3 + 4. All channels off: 0x68 must vanish; then CHIP_ID of I8.
    let outcome = {
        let mut i2c = TimeoutI2c::new(port.bus());
        probe_trunk(&mut i2c).await
    };
    let mut trunk_imu_ok = false;
    match outcome {
        Ok((readback, found, trunk_id)) => {
            check!(
                report,
                readback == 0,
                "all channels off: control register reads back {:#04x} (expected 0x00)",
                readback
            );
            check!(
                report,
                !found.contains(CHANNEL_IMU.address()),
                "all channels off: 0x68 absent (found {})",
                found
            );
            trunk_imu_ok = match trunk_id {
                Some(v) => check!(
                    report,
                    v == bmi160::CHIP_ID_VALUE,
                    "I8: BMI160 at 0x69 on the trunk, CHIP_ID = {:#04x} (expected 0xd1)",
                    v
                ),
                None => check!(
                    report,
                    false,
                    "I8: nothing at 0x69 on the trunk: check its wiring and that its SAO strap is high"
                ),
            };
        }
        Err(e) => {
            fail!(report, "all channels off: I2C error {:?}", e);
            let state = port.recover().await;
            note!("bus recovery: {:?}", state);
        }
    }

    // Verdict.
    if failing_channels == 0 {
        note!("verdict: all 8 mux channels OK");
    } else {
        let mut list: String<32> = String::new();
        list_channels(failing_channels, &mut list);
        note!("verdict: failing channels: {}", list);
    }
    if trunk_missing != 0 {
        let mut list: String<32> = String::new();
        list_channels(trunk_missing, &mut list);
        note!(
            "verdict: 0x69/0x70 not visible with channel(s) {} selected",
            list
        );
    }
    note!(
        "verdict: I8 (0x69) {}",
        if trunk_imu_ok { "OK" } else { "FAILED" }
    );

    let verdict = report.finish();
    report::idle_blink(&mut led2, verdict).await
}
