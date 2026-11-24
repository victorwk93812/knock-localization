//! I2C address probing and bus scanning.
//!
//! A device is probed with a **1-byte read**: the address byte is ACKed by a
//! present device and NACKed by an empty address. A read is used instead of the
//! classic zero-length write because zero-length transfers take a different,
//! blocking code path in the STM32 HAL; the 1-byte read is the ordinary
//! well-trodden path.
//!
//! Side effects of the read on the devices of this project: the TCA9548A
//! returns its control register; the BMI160 and LSM6DSL return the register at
//! their current address pointer. That is harmless except that reading the
//! BMI160's ERR_REG clears its flags, so do not scan between a configuration
//! step and the error check.

use embedded_hal::i2c::{Error as _, ErrorKind};
use embedded_hal_async::i2c::I2c;

/// First address of the standard scan range (0x00–0x07 are reserved).
pub const SCAN_FIRST: u8 = 0x08;
/// Last address of the standard scan range (0x78–0x7F are reserved).
pub const SCAN_LAST: u8 = 0x77;

/// A set of 7-bit I2C addresses (0x00–0x7F).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct AddressSet(u128);

impl AddressSet {
    /// The empty set.
    pub const fn new() -> Self {
        Self(0)
    }

    /// Build a set from a list of addresses (addresses above 0x7F are ignored).
    pub const fn from_slice(addrs: &[u8]) -> Self {
        let mut set = Self(0);
        let mut i = 0;
        while i < addrs.len() {
            set.insert(addrs[i]);
            i += 1;
        }
        set
    }

    /// Add `addr`. Addresses above 0x7F are ignored.
    pub const fn insert(&mut self, addr: u8) {
        if addr < 0x80 {
            self.0 |= 1u128 << addr;
        }
    }

    /// Remove `addr` if present.
    pub const fn remove(&mut self, addr: u8) {
        if addr < 0x80 {
            self.0 &= !(1u128 << addr);
        }
    }

    /// True if `addr` is in the set.
    pub const fn contains(&self, addr: u8) -> bool {
        addr < 0x80 && self.0 & (1u128 << addr) != 0
    }

    /// Number of addresses in the set.
    pub const fn len(&self) -> usize {
        self.0.count_ones() as usize
    }

    /// True if the set is empty.
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// The addresses in `self` that are not in `other`.
    pub const fn difference(&self, other: &AddressSet) -> AddressSet {
        AddressSet(self.0 & !other.0)
    }

    /// Iterate over the addresses in ascending order.
    pub fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        (0u8..0x80).filter(|a| self.contains(*a))
    }
}

/// Formats as a list of hex addresses: `[0x68, 0x69, 0x70]`.
impl core::fmt::Debug for AddressSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("[")?;
        for (i, a) in self.iter().enumerate() {
            if i != 0 {
                f.write_str(", ")?;
            }
            write!(f, "{a:#04x}")?;
        }
        f.write_str("]")
    }
}

impl core::fmt::Display for AddressSet {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for AddressSet {
    fn format(&self, f: defmt::Formatter<'_>) {
        defmt::write!(f, "[");
        for (i, a) in self.iter().enumerate() {
            if i != 0 {
                defmt::write!(f, ", ");
            }
            defmt::write!(f, "{=u8:#04x}", a);
        }
        defmt::write!(f, "]");
    }
}

/// Probe one address with a 1-byte read.
///
/// Returns `Ok(true)` if the device ACKed, `Ok(false)` if the address was
/// NACKed, and `Err` for anything else (bus error, arbitration loss, timeout):
/// those mean the bus itself is in trouble, not that the address is empty.
pub async fn probe<I: I2c>(i2c: &mut I, addr: u8) -> Result<bool, I::Error> {
    let mut b = [0u8; 1];
    match i2c.read(addr, &mut b).await {
        Ok(()) => Ok(true),
        Err(e) if matches!(e.kind(), ErrorKind::NoAcknowledge(_)) => Ok(false),
        Err(e) => Err(e),
    }
}

/// Probe every address in `first..=last` and return the set that answered.
///
/// Stops at the first error that is not a NACK and returns it.
pub async fn scan_range<I: I2c>(i2c: &mut I, first: u8, last: u8) -> Result<AddressSet, I::Error> {
    let mut found = AddressSet::new();
    for addr in first..=last.min(0x7F) {
        if probe(i2c, addr).await? {
            found.insert(addr);
        }
    }
    Ok(found)
}

/// Scan the standard range 0x08–0x77.
pub async fn scan<I: I2c>(i2c: &mut I) -> Result<AddressSet, I::Error> {
    scan_range(i2c, SCAN_FIRST, SCAN_LAST).await
}

#[cfg(test)]
mod tests {
    use embedded_hal::i2c::NoAcknowledgeSource;
    use embedded_hal_mock::eh1::i2c::{Mock, Transaction};

    use super::*;
    use crate::testutil::block_on;

    fn nack(addr: u8) -> Transaction {
        Transaction::read(addr, vec![0])
            .with_error(ErrorKind::NoAcknowledge(NoAcknowledgeSource::Unknown))
    }

    fn ack(addr: u8) -> Transaction {
        Transaction::read(addr, vec![0])
    }

    #[test]
    fn address_set_basics() {
        let mut s = AddressSet::new();
        assert!(s.is_empty());
        s.insert(0x70);
        s.insert(0x68);
        s.insert(0x68);
        s.insert(0x80); // ignored
        s.insert(0xFF); // ignored
        assert_eq!(s.len(), 2);
        assert!(s.contains(0x68) && s.contains(0x70));
        assert!(!s.contains(0x69) && !s.contains(0x80));
        assert_eq!(s.iter().collect::<Vec<_>>(), vec![0x68, 0x70]);
        s.remove(0x68);
        assert_eq!(s, AddressSet::from_slice(&[0x70]));
    }

    #[test]
    fn address_set_boundaries_and_difference() {
        let s = AddressSet::from_slice(&[0x00, 0x7F, 0x69]);
        assert!(s.contains(0x00) && s.contains(0x7F));
        let d = s.difference(&AddressSet::from_slice(&[0x69, 0x10]));
        assert_eq!(d, AddressSet::from_slice(&[0x00, 0x7F]));
    }

    #[test]
    fn address_set_formats_as_hex_list() {
        let s = AddressSet::from_slice(&[0x70, 0x08, 0x69]);
        assert_eq!(format!("{s}"), "[0x08, 0x69, 0x70]");
        assert_eq!(format!("{:?}", AddressSet::new()), "[]");
    }

    #[test]
    fn probe_distinguishes_ack_nack_and_bus_errors() {
        let mut i2c = Mock::new(&[
            ack(0x68),
            nack(0x69),
            Transaction::read(0x6A, vec![0]).with_error(ErrorKind::Bus),
        ]);
        assert_eq!(block_on(probe(&mut i2c, 0x68)), Ok(true));
        assert_eq!(block_on(probe(&mut i2c, 0x69)), Ok(false));
        assert_eq!(block_on(probe(&mut i2c, 0x6A)), Err(ErrorKind::Bus));
        i2c.done();
    }

    #[test]
    fn scan_covers_0x08_to_0x77_and_collects_acks() {
        let present = [0x68u8, 0x69, 0x70];
        let expected: Vec<Transaction> = (0x08u8..=0x77)
            .map(|a| {
                if present.contains(&a) {
                    ack(a)
                } else {
                    nack(a)
                }
            })
            .collect();
        assert_eq!(expected.len(), 112);
        let mut i2c = Mock::new(&expected);
        let found = block_on(scan(&mut i2c)).unwrap();
        assert_eq!(found, AddressSet::from_slice(&present));
        i2c.done();
    }

    #[test]
    fn scan_stops_at_the_first_bus_error() {
        let mut i2c = Mock::new(&[
            nack(0x10),
            ack(0x11),
            Transaction::read(0x12, vec![0]).with_error(ErrorKind::ArbitrationLoss),
        ]);
        assert_eq!(
            block_on(scan_range(&mut i2c, 0x10, 0x20)),
            Err(ErrorKind::ArbitrationLoss)
        );
        i2c.done();
    }

    #[test]
    fn scan_range_clamps_to_seven_bit_addresses() {
        let mut i2c = Mock::new(&[nack(0x7E), ack(0x7F)]);
        let found = block_on(scan_range(&mut i2c, 0x7E, 0xFF)).unwrap();
        assert_eq!(found, AddressSet::from_slice(&[0x7F]));
        i2c.done();
    }
}
