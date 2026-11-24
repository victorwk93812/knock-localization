//! I2C timeout wrapper.
//!
//! [`TimeoutI2c`] wraps any async I2C bus and bounds every transfer with
//! [`embassy_time::with_timeout`], so a stuck bus (SDA held low, a slave
//! stretching the clock forever, a missing pull-up) produces
//! [`BusError::Timeout`] instead of hanging the test.
//!
//! After a timeout the transfer was abandoned half-way: assume the bus is in an
//! unknown state and run [`crate::board::I2c1Port::recover`] before using it
//! again.
//!
//! Note: embassy-stm32's I2C driver also has its own timeout
//! (`i2c::Config::timeout`, set in [`crate::board`]). This wrapper is the outer
//! guard and works for any `embedded-hal-async` bus. It cannot interrupt code
//! that never yields; the HAL's blocking zero-length transfers are covered only
//! by the HAL's own timeout, which is one reason [`crate::scan`] probes with a
//! 1-byte read.

use embassy_time::{Duration, with_timeout};
use embedded_hal::i2c::{ErrorKind, ErrorType, Operation};
use embedded_hal_async::i2c::I2c;

/// Default per-transfer timeout.
///
/// The longest transfer in the project is a full 1024-byte FIFO burst, about
/// 23 ms at 400 kHz and 93 ms at 100 kHz.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(200);

/// Error of a [`TimeoutI2c`] transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum BusError<E> {
    /// The underlying bus reported an error (NACK, bus error, ...).
    I2c(E),
    /// The transfer did not finish in time and was abandoned.
    Timeout,
}

impl<E: embedded_hal::i2c::Error> embedded_hal::i2c::Error for BusError<E> {
    fn kind(&self) -> ErrorKind {
        match self {
            BusError::I2c(e) => e.kind(),
            BusError::Timeout => ErrorKind::Other,
        }
    }
}

/// An I2C bus whose transfers fail with [`BusError::Timeout`] instead of
/// hanging.
pub struct TimeoutI2c<I> {
    inner: I,
    timeout: Duration,
}

impl<I> TimeoutI2c<I> {
    /// Wrap `inner`, bounding every transfer by [`DEFAULT_TIMEOUT`].
    pub const fn new(inner: I) -> Self {
        Self::with_timeout(inner, DEFAULT_TIMEOUT)
    }

    /// Wrap `inner`, bounding every transfer by `timeout`.
    pub const fn with_timeout(inner: I, timeout: Duration) -> Self {
        Self { inner, timeout }
    }

    /// The per-transfer timeout.
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Change the per-transfer timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    /// Unwrap the inner bus.
    pub fn into_inner(self) -> I {
        self.inner
    }
}

impl<I: I2c> ErrorType for TimeoutI2c<I> {
    type Error = BusError<I::Error>;
}

/// Flatten "timed out" and "bus error" into one result.
fn flatten<E>(r: Result<Result<(), E>, embassy_time::TimeoutError>) -> Result<(), BusError<E>> {
    match r {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(BusError::I2c(e)),
        Err(embassy_time::TimeoutError) => Err(BusError::Timeout),
    }
}

// Each method forwards to the same method of the inner bus (rather than
// routing everything through `transaction`) so the HAL's specialised DMA paths
// are used.
impl<I: I2c> I2c for TimeoutI2c<I> {
    async fn read(&mut self, address: u8, read: &mut [u8]) -> Result<(), Self::Error> {
        flatten(with_timeout(self.timeout, self.inner.read(address, read)).await)
    }

    async fn write(&mut self, address: u8, write: &[u8]) -> Result<(), Self::Error> {
        flatten(with_timeout(self.timeout, self.inner.write(address, write)).await)
    }

    async fn write_read(
        &mut self,
        address: u8,
        write: &[u8],
        read: &mut [u8],
    ) -> Result<(), Self::Error> {
        flatten(with_timeout(self.timeout, self.inner.write_read(address, write, read)).await)
    }

    async fn transaction(
        &mut self,
        address: u8,
        operations: &mut [Operation<'_>],
    ) -> Result<(), Self::Error> {
        flatten(with_timeout(self.timeout, self.inner.transaction(address, operations)).await)
    }
}
