//! Shared support code for the knock-localization bench-test binaries
//! (`src/bin/tNN_*.rs`) on the B-L475E-IOT01A Discovery kit.
//!
//! - [`board`]: clocks, pins and peripherals ([`board::Board::init`]).
//! - [`bus`]: [`bus::TimeoutI2c`], an I2C wrapper that turns a hung transfer
//!   into an error.
//! - [`recovery`]: I2C bus recovery on the real pins.
//! - [`report`]: the common test-report shape (banner, PASS/FAIL lines,
//!   RESULT line), the CSV line writer and the final LED idle loop.
//! - [`scan`]: address probe and bus scan (re-exported from `knock-drivers`).
//! - [`imu`]: the nine-sensor BMI160 array (bring-up, round-robin FIFO readout).
//!
//! The sensor drivers live in the HAL-independent `knock-drivers` crate,
//! re-exported here as [`drivers`].
//!
//! Linking this crate also pulls in the defmt RTT transport and the panic
//! handler, so a binary only needs `use knock_bench as _;` or any other use of
//! the crate. See `firmware/README.md` for a template.

#![no_std]
#![warn(missing_docs)]

// Global logger and panic handler for every binary.
use {defmt_rtt as _, panic_probe as _};

pub mod board;
pub mod bus;
pub mod recovery;
pub mod report;

pub mod imu;

pub use knock_drivers as drivers;
pub use knock_drivers::scan;
