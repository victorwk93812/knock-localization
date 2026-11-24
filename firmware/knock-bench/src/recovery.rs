//! I2C bus recovery on the real pins.
//!
//! The sequence itself (release, 9 SCL pulses, STOP) is
//! [`knock_drivers::recovery::recover_bus`], which is unit-tested on the host.
//! This module drives it with embassy GPIOs.
//!
//! # Ownership: how the pins get "handed back"
//!
//! embassy-stm32's `I2c` owns its pins for its whole lifetime and offers no way
//! to borrow them back, so recovery cannot run while an `I2c` exists. Instead
//! of the driver, the *board* keeps the pins: [`crate::board::I2c1Port`] owns
//! the I2C1 peripheral, SCL, SDA and both DMA channels, and lends them out.
//!
//! - `port.bus()` builds an `I2c<'_>` from re-borrowed peripherals;
//! - dropping that `I2c` disables the peripheral and disconnects the pins;
//! - `port.recover().await` then re-borrows SCL/SDA as open-drain GPIOs and
//!   calls [`recover_pins`]; when it returns the GPIOs are dropped too;
//! - `port.bus()` builds a fresh, fully re-initialised `I2c`.
//!
//! The borrow checker enforces the order: `recover` needs `&mut port`, which is
//! unavailable while a bus borrowed from it is alive.
//!
//! ```ignore
//! let mut port = board.i2c1;
//! loop {
//!     let mut i2c = TimeoutI2c::new(port.bus());
//!     let outcome = run_test(&mut i2c).await;
//!     drop(i2c);
//!     if outcome.is_err() {
//!         let state = port.recover().await;
//!         defmt::warn!("bus recovery: {}", state);
//!     }
//! }
//! ```

use embassy_stm32::Peri;
use embassy_stm32::gpio::{Flex, Pin, Pull, Speed};
use embassy_time::Delay;
pub use knock_drivers::recovery::{BusState, CLOCK_PULSES, DEFAULT_HALF_PERIOD_US};

/// Run the bus-recovery sequence on the given SCL and SDA pins.
///
/// The pins are driven as open-drain GPIOs with the internal pull-ups enabled
/// (so the lines can rise even if no module with pull-ups is attached), at
/// [`DEFAULT_HALF_PERIOD_US`] per half clock period (about 2.3 ms in total).
/// On return both pins are disconnected (analog mode) and can be given to a
/// new `I2c`.
pub async fn recover_pins(scl: Peri<'_, impl Pin>, sda: Peri<'_, impl Pin>) -> BusState {
    let mut scl = Flex::new(scl);
    let mut sda = Flex::new(sda);

    // Set the output latches high before enabling the open-drain drivers, so
    // switching the mode does not pull a line low by itself.
    scl.set_high();
    sda.set_high();
    scl.set_as_input_output_pull(Speed::Low, Pull::Up);
    sda.set_as_input_output_pull(Speed::Low, Pull::Up);

    let result = knock_drivers::recovery::recover_bus(
        &mut scl,
        &mut sda,
        &mut Delay,
        DEFAULT_HALF_PERIOD_US,
    )
    .await;

    match result {
        Ok(state) => state,
        // GPIO operations cannot fail on this HAL.
        Err(never) => match never {},
    }
}
