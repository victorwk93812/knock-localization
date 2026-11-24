//! Minimal LSM6DSL accelerometer driver (the Discovery board's onboard IMU,
//! used as the table reference).
//!
//! The LSM6DSL datasheet is not in the project's `references/` directory. The
//! register addresses, the ODR / full-scale encodings and the bit positions
//! below were cross-checked against ST's own driver (`lsm6dsl_reg.h` in
//! STMicroelectronics/lsm6dsl-pid), not against the datasheet itself.

use embedded_hal_async::i2c::I2c;

use crate::Accel;

/// I2C address with SA0 low, as wired on the B-L475E-IOT01A.
pub const DEFAULT_ADDRESS: u8 = 0x6A;
/// Value of the WHO_AM_I register.
pub const WHO_AM_I_VALUE: u8 = 0x6A;

/// Register addresses.
pub mod reg {
    /// WHO_AM_I, reads 0x6A.
    pub const WHO_AM_I: u8 = 0x0F;
    /// CTRL1_XL: accelerometer ODR (bits 7:4), full scale (bits 3:2), filter.
    pub const CTRL1_XL: u8 = 0x10;
    /// CTRL3_C: BOOT, BDU (bit 6), IF_INC (bit 2), SW_RESET (bit 0).
    pub const CTRL3_C: u8 = 0x12;
    /// STATUS_REG: XLDA (bit 0) = new accelerometer data.
    pub const STATUS_REG: u8 = 0x1E;
    /// OUTX_L_XL: accelerometer X LSB; X/Y/Z little-endian i16 up to 0x2D.
    pub const OUTX_L_XL: u8 = 0x28;
}

/// CTRL1_XL value: ODR_XL = 0b0110 (416 Hz), FS_XL = 0b01 (±16 g), default
/// analog bandwidth.
pub const CTRL1_XL_416HZ_16G: u8 = 0x64;
/// CTRL3_C value: BDU (block data update) and IF_INC (register address
/// auto-increment, the reset default) set.
pub const CTRL3_C_BDU_IF_INC: u8 = 0x44;

/// Output data rate set by [`Lsm6dsl::init_accel`], in Hz.
pub const ODR_HZ: u32 = 416;

/// Sensitivity at ±16 g in micro-g per LSB.
///
// UNVERIFIED: 0.488 mg/LSB is quoted from memory of the LSM6DSL datasheet
// (mechanical characteristics, LA_So at FS = ±16 g); the datasheet is not in
// `references/`. t00_board's 0.8–1.2 g gravity check would catch a wrong value.
pub const MICRO_G_PER_LSB_16G: i32 = 488;

/// Convert a raw ±16 g sample to milli-g (integer, truncating toward zero).
pub const fn raw_to_mg_16g(raw: i16) -> i32 {
    raw as i32 * MICRO_G_PER_LSB_16G / 1000
}

/// Convert a raw (possibly averaged) ±16 g value to milli-g.
pub fn raw_to_mg_16g_f32(raw: f32) -> f32 {
    raw * (MICRO_G_PER_LSB_16G as f32 / 1000.0)
}

/// Handle for one LSM6DSL. Holds only the address; see the crate docs for the
/// bus-ownership model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Lsm6dsl {
    addr: u8,
}

impl Lsm6dsl {
    /// Create a handle for the device at `i2c_addr` (normally
    /// [`DEFAULT_ADDRESS`]).
    pub const fn new(i2c_addr: u8) -> Self {
        Self { addr: i2c_addr }
    }

    /// The I2C address this handle talks to.
    pub const fn address(&self) -> u8 {
        self.addr
    }

    /// Read WHO_AM_I (0x0F). An LSM6DSL returns [`WHO_AM_I_VALUE`] (0x6A).
    pub async fn who_am_i<I: I2c>(&self, i2c: &mut I) -> Result<u8, I::Error> {
        let mut b = [0u8; 1];
        i2c.write_read(self.addr, &[reg::WHO_AM_I], &mut b).await?;
        Ok(b[0])
    }

    /// Minimal accelerometer set-up: CTRL3_C ← 0x44 (BDU + IF_INC), then
    /// CTRL1_XL ← 0x64 (416 Hz, ±16 g). The gyroscope stays powered down.
    ///
    /// The first samples after power-up are not settled; discard a few (or
    /// wait ~20 ms) before using the data.
    pub async fn init_accel<I: I2c>(&self, i2c: &mut I) -> Result<(), I::Error> {
        i2c.write(self.addr, &[reg::CTRL3_C, CTRL3_C_BDU_IF_INC])
            .await?;
        i2c.write(self.addr, &[reg::CTRL1_XL, CTRL1_XL_416HZ_16G])
            .await
    }

    /// Read the latest accelerometer sample (OUTX_L_XL..OUTZ_H_XL).
    pub async fn read_accel<I: I2c>(&self, i2c: &mut I) -> Result<Accel, I::Error> {
        let mut b = [0u8; Accel::WIRE_LEN];
        i2c.write_read(self.addr, &[reg::OUTX_L_XL], &mut b).await?;
        Ok(Accel::from_le_bytes(b))
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
    fn register_values_match_bitfields() {
        // ODR_XL = 6 (416 Hz) in bits 7:4, FS_XL = 1 (±16 g) in bits 3:2.
        assert_eq!(CTRL1_XL_416HZ_16G, (6 << 4) | (1 << 2));
        // BDU is bit 6, IF_INC is bit 2.
        assert_eq!(CTRL3_C_BDU_IF_INC, (1 << 6) | (1 << 2));
    }

    #[test]
    fn who_am_i_reads_0x0f() {
        let mut i2c = Mock::new(&[Transaction::write_read(A, vec![0x0F], vec![0x6A])]);
        let imu = Lsm6dsl::new(A);
        assert_eq!(imu.address(), 0x6A);
        assert_eq!(block_on(imu.who_am_i(&mut i2c)), Ok(WHO_AM_I_VALUE));
        i2c.done();
    }

    #[test]
    fn init_writes_ctrl3_then_ctrl1() {
        let mut i2c = Mock::new(&[
            Transaction::write(A, vec![0x12, 0x44]),
            Transaction::write(A, vec![0x10, 0x64]),
        ]);
        let imu = Lsm6dsl::new(A);
        assert_eq!(block_on(imu.init_accel(&mut i2c)), Ok(()));
        i2c.done();
    }

    #[test]
    fn init_stops_at_the_first_bus_error() {
        let mut i2c =
            Mock::new(&[Transaction::write(A, vec![0x12, 0x44]).with_error(ErrorKind::Other)]);
        let imu = Lsm6dsl::new(A);
        assert_eq!(block_on(imu.init_accel(&mut i2c)), Err(ErrorKind::Other));
        i2c.done();
    }

    #[test]
    fn read_accel_reads_six_bytes_from_0x28() {
        let mut i2c = Mock::new(&[Transaction::write_read(
            A,
            vec![0x28],
            vec![0x02, 0x00, 0xFE, 0xFF, 0x01, 0x08],
        )]);
        let imu = Lsm6dsl::new(A);
        assert_eq!(
            block_on(imu.read_accel(&mut i2c)),
            Ok(Accel::new(2, -2, 2049))
        );
        i2c.done();
    }

    #[test]
    fn mg_conversion_at_16g() {
        // 1 g = 1000 / 0.488 ≈ 2049 LSB.
        assert_eq!(raw_to_mg_16g(2049), 999);
        assert_eq!(raw_to_mg_16g(-2049), -999);
        assert_eq!(raw_to_mg_16g(i16::MAX), 15_990);
        assert!((raw_to_mg_16g_f32(2049.0) - 999.912).abs() < 1e-2);
    }
}
