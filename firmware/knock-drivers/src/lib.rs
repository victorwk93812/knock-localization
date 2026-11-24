//! HAL-independent sensor drivers and signal helpers for the knock-localization
//! bench.
//!
//! Nothing in this crate depends on a microcontroller HAL: the drivers are
//! generic over [`embedded_hal_async::i2c::I2c`], so the register sequences are
//! unit-tested on the host with `embedded-hal-mock`.
//!
//! # Bus ownership
//!
//! Nine BMI160s, the TCA9548A multiplexer and (on another bus) the LSM6DSL are
//! all driven from one task, so the drivers do **not** own the bus. Each driver
//! is a small `Copy` handle holding only the device address, and every method
//! borrows the bus for the duration of the call:
//!
//! ```ignore
//! let mux = Tca9548a::new(tca9548a::DEFAULT_ADDRESS);
//! let imu = Bmi160::new(bmi160::ADDR_SDO_LOW);
//! mux.select_channel(&mut i2c, 3).await?;
//! let id = imu.chip_id(&mut i2c).await?;
//! ```
//!
//! # Modules
//!
//! - [`bmi160`]: BMI160 accelerometer (accel-only, 400 Hz, ±16 g, headerless FIFO).
//! - [`tca9548a`]: TCA9548A 8-channel I2C multiplexer.
//! - [`lsm6dsl`]: minimal LSM6DSL accelerometer (onboard table reference).
//! - [`fifo`]: parser for headerless 6-byte accelerometer FIFO frames.
//! - [`stats`]: mean / RMS / peak / onset helpers (pure functions, no alloc).
//! - [`recovery`]: pin-level I2C bus recovery (9 SCL pulses + STOP).
//! - [`scan`]: address probing, bus scan and the [`scan::AddressSet`] result type.
//! - [`health`], [`rate`], [`running`], [`knock`]: bench-test logic for the
//!   accelerometer array (error counters and outages, rate / FIFO overflow,
//!   streaming statistics, knock trigger and window extraction).
//! - [`piezo`]: piezo channel de-interleaving, trigger, window capture, onset and ring-down.

#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

pub mod bmi160;
pub mod fifo;
pub mod lsm6dsl;
pub mod piezo;
pub mod recovery;
pub mod scan;
pub mod stats;
pub mod tca9548a;

pub mod health;
pub mod knock;
pub mod rate;
pub mod running;

/// One raw accelerometer sample (sensor LSB, signed, as read from the device).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Accel {
    /// X axis, raw LSB.
    pub x: i16,
    /// Y axis, raw LSB.
    pub y: i16,
    /// Z axis, raw LSB.
    pub z: i16,
}

impl Accel {
    /// Size of one sample on the wire: X, Y, Z as little-endian `i16`.
    pub const WIRE_LEN: usize = 6;

    /// Build a sample from three axis values.
    pub const fn new(x: i16, y: i16, z: i16) -> Self {
        Self { x, y, z }
    }

    /// Decode a sample from 6 bytes: X LSB, X MSB, Y LSB, Y MSB, Z LSB, Z MSB.
    ///
    /// This is the layout of both the BMI160 (data registers and headerless FIFO
    /// frames) and the LSM6DSL output registers.
    pub const fn from_le_bytes(b: [u8; 6]) -> Self {
        Self {
            x: i16::from_le_bytes([b[0], b[1]]),
            y: i16::from_le_bytes([b[2], b[3]]),
            z: i16::from_le_bytes([b[4], b[5]]),
        }
    }

    /// The three axes as an array `[x, y, z]`.
    pub const fn to_array(self) -> [i16; 3] {
        [self.x, self.y, self.z]
    }

    /// Euclidean norm in raw LSB.
    pub fn magnitude(self) -> f32 {
        stats::magnitude(self.x as f32, self.y as f32, self.z as f32)
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Shared helpers for the host-side unit tests.

    use embedded_hal_async::delay::DelayNs;

    /// A delay that returns immediately and records every requested duration.
    #[derive(Default)]
    pub struct RecordingDelay {
        /// Requested delays in nanoseconds, in call order.
        pub calls_ns: Vec<u64>,
    }

    impl RecordingDelay {
        /// Sum of all requested delays in nanoseconds.
        pub fn total_ns(&self) -> u64 {
            self.calls_ns.iter().sum()
        }
    }

    impl DelayNs for RecordingDelay {
        async fn delay_ns(&mut self, ns: u32) {
            self.calls_ns.push(u64::from(ns));
        }
    }

    /// Run a future to completion on the test thread.
    pub fn block_on<F: core::future::Future>(fut: F) -> F::Output {
        embassy_futures::block_on(fut)
    }
}

#[cfg(test)]
mod tests {
    use super::Accel;

    #[test]
    fn accel_decodes_little_endian() {
        let a = Accel::from_le_bytes([0x01, 0x00, 0xFF, 0xFF, 0x00, 0x08]);
        assert_eq!(a, Accel::new(1, -1, 2048));
        assert_eq!(a.to_array(), [1, -1, 2048]);
    }

    #[test]
    fn accel_magnitude() {
        let a = Accel::new(3, -4, 12);
        assert!((a.magnitude() - 13.0).abs() < 1e-5);
    }
}
