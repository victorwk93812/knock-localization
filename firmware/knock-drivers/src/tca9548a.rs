//! TCA9548A 8-channel I2C multiplexer.
//!
//! The device has a single 8-bit control register with no register address:
//! writing one byte sets it, reading one byte returns it. Bit *k* connects
//! downstream channel *k* to the upstream bus; several bits may be set at once.

use embedded_hal_async::i2c::I2c;

/// I2C address with A2..A0 tied low (the module default).
pub const DEFAULT_ADDRESS: u8 = 0x70;
/// Number of downstream channels.
pub const CHANNELS: u8 = 8;

/// Errors from the multiplexer driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// The I2C transfer failed.
    Bus(E),
    /// A channel number above 7 was requested; nothing was written.
    InvalidChannel(u8),
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Bus(e)
    }
}

/// Handle for one TCA9548A. Holds only the address; see the crate docs for the
/// bus-ownership model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Tca9548a {
    addr: u8,
}

impl Tca9548a {
    /// Create a handle for the multiplexer at `i2c_addr` (0x70–0x77).
    pub const fn new(i2c_addr: u8) -> Self {
        Self { addr: i2c_addr }
    }

    /// The I2C address this handle talks to.
    pub const fn address(&self) -> u8 {
        self.addr
    }

    /// Write the control register: bit *k* of `mask` enables channel *k*.
    pub async fn select<I: I2c>(&self, i2c: &mut I, mask: u8) -> Result<(), I::Error> {
        i2c.write(self.addr, &[mask]).await
    }

    /// Enable exactly one channel, `channel` in 0..=7.
    pub async fn select_channel<I: I2c>(
        &self,
        i2c: &mut I,
        channel: u8,
    ) -> Result<(), Error<I::Error>> {
        if channel >= CHANNELS {
            return Err(Error::InvalidChannel(channel));
        }
        self.select(i2c, 1 << channel).await?;
        Ok(())
    }

    /// Read the control register back.
    pub async fn readback<I: I2c>(&self, i2c: &mut I) -> Result<u8, I::Error> {
        let mut b = [0u8; 1];
        i2c.read(self.addr, &mut b).await?;
        Ok(b[0])
    }

    /// Disconnect every downstream channel (control register ← 0).
    pub async fn disable_all<I: I2c>(&self, i2c: &mut I) -> Result<(), I::Error> {
        self.select(i2c, 0).await
    }
}

#[cfg(test)]
mod tests {
    use embedded_hal::i2c::ErrorKind;
    use embedded_hal_mock::eh1::i2c::{Mock, Transaction};

    use super::*;
    use crate::testutil::block_on;

    const A: u8 = DEFAULT_ADDRESS;

    #[test]
    fn select_writes_the_mask_as_a_single_byte() {
        let mut i2c = Mock::new(&[Transaction::write(A, vec![0b1010_0001])]);
        let mux = Tca9548a::new(A);
        assert_eq!(block_on(mux.select(&mut i2c, 0b1010_0001)), Ok(()));
        i2c.done();
    }

    #[test]
    fn select_channel_writes_one_hot_masks() {
        let expected: Vec<Transaction> = (0..8)
            .map(|k| Transaction::write(A, vec![1 << k]))
            .collect();
        let mut i2c = Mock::new(&expected);
        let mux = Tca9548a::new(A);
        for k in 0..8 {
            assert_eq!(block_on(mux.select_channel(&mut i2c, k)), Ok(()));
        }
        i2c.done();
    }

    #[test]
    fn select_channel_rejects_out_of_range_without_bus_traffic() {
        let mut i2c = Mock::new(&[]);
        let mux = Tca9548a::new(A);
        assert_eq!(
            block_on(mux.select_channel(&mut i2c, 8)),
            Err(Error::InvalidChannel(8))
        );
        i2c.done();
    }

    #[test]
    fn readback_reads_one_byte_without_a_register_address() {
        let mut i2c = Mock::new(&[Transaction::read(A, vec![0x08])]);
        let mux = Tca9548a::new(A);
        assert_eq!(block_on(mux.readback(&mut i2c)), Ok(0x08));
        i2c.done();
    }

    #[test]
    fn disable_all_writes_zero() {
        let mut i2c = Mock::new(&[Transaction::write(0x71, vec![0x00])]);
        let mux = Tca9548a::new(0x71);
        assert_eq!(mux.address(), 0x71);
        assert_eq!(block_on(mux.disable_all(&mut i2c)), Ok(()));
        i2c.done();
    }

    #[test]
    fn bus_errors_are_propagated() {
        let mut i2c = Mock::new(&[
            Transaction::write(A, vec![0x04]).with_error(ErrorKind::Other),
            Transaction::read(A, vec![0x00]).with_error(ErrorKind::Other),
        ]);
        let mux = Tca9548a::new(A);
        assert_eq!(
            block_on(mux.select_channel(&mut i2c, 2)),
            Err(Error::Bus(ErrorKind::Other))
        );
        assert_eq!(block_on(mux.readback(&mut i2c)), Err(ErrorKind::Other));
        i2c.done();
    }
}
