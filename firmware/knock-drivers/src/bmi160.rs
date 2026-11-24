//! BMI160 accelerometer driver (I2C, accelerometer only).
//!
//! Register addresses, values and timings are taken from the Bosch datasheet
//! BST-BMI160-DS000-09 (rev. 1.0, November 2020); the section each one comes
//! from is noted next to it.
//!
//! The project configuration set by [`Bmi160::init_accel_fifo`] is:
//! accelerometer in normal mode, gyroscope suspended, ODR 400 Hz with the
//! normal filter, range ±16 g, FIFO storing accelerometer frames only in
//! headerless mode (6 bytes per frame, see [`crate::fifo`]).

use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::i2c::I2c;

use crate::Accel;

/// I2C address with the SDO/SAO pin tied to GND (datasheet §3.2.3). Sensors I0–I7.
pub const ADDR_SDO_LOW: u8 = 0x68;
/// I2C address with the SDO/SAO pin tied to VDDIO (datasheet §3.2.3). Sensor I8.
pub const ADDR_SDO_HIGH: u8 = 0x69;

/// Value of the CHIP_ID register (datasheet §2.11.1).
pub const CHIP_ID_VALUE: u8 = 0xD1;

/// Register addresses (datasheet §2.11, register map figure 20).
pub mod reg {
    /// CHIP_ID, reads 0xD1.
    pub const CHIP_ID: u8 = 0x00;
    /// ERR_REG: error flags, cleared on read.
    pub const ERR_REG: u8 = 0x02;
    /// PMU_STATUS: current power mode of accel / gyro / mag interface.
    pub const PMU_STATUS: u8 = 0x03;
    /// DATA_14: accelerometer X LSB; X/Y/Z as little-endian i16 up to 0x17.
    pub const DATA_ACC_X_LSB: u8 = 0x12;
    /// SENSORTIME_0: 24-bit free-running counter, LSB first, up to 0x1A.
    pub const SENSORTIME_0: u8 = 0x18;
    /// STATUS: data-ready and self-test flags.
    pub const STATUS: u8 = 0x1B;
    /// FIFO_LENGTH_0: FIFO fill level in bytes, 11 bits, LSB first (0x22–0x23).
    pub const FIFO_LENGTH_0: u8 = 0x22;
    /// FIFO_DATA: burst-read port of the FIFO (the address does not increment).
    pub const FIFO_DATA: u8 = 0x24;
    /// ACC_CONF: accelerometer ODR, bandwidth and undersampling.
    pub const ACC_CONF: u8 = 0x40;
    /// ACC_RANGE: accelerometer g-range.
    pub const ACC_RANGE: u8 = 0x41;
    /// FIFO_DOWNS: FIFO down-sampling and filtered/unfiltered data selection.
    pub const FIFO_DOWNS: u8 = 0x45;
    /// FIFO_CONFIG_0: FIFO watermark level (unit: 4 bytes).
    pub const FIFO_CONFIG_0: u8 = 0x46;
    /// FIFO_CONFIG_1: FIFO sensor enables and frame format.
    pub const FIFO_CONFIG_1: u8 = 0x47;
    /// CMD: command register (power modes, FIFO flush, soft reset).
    pub const CMD: u8 = 0x7E;
}

/// Opcodes for the CMD register 0x7E (datasheet §2.11.38).
pub mod cmd {
    /// `acc_set_pmu_mode` = 0b0001_00nn with nn = 0b01: accelerometer normal mode.
    pub const ACC_NORMAL: u8 = 0x11;
    /// `acc_set_pmu_mode` with nn = 0b00: accelerometer suspend.
    pub const ACC_SUSPEND: u8 = 0x10;
    /// `gyr_set_pmu_mode` = 0b0001_01nn with nn = 0b00: gyroscope suspend.
    pub const GYR_SUSPEND: u8 = 0x14;
    /// `fifo_flush`: clear the FIFO contents, keep the FIFO configuration.
    pub const FIFO_FLUSH: u8 = 0xB0;
    /// `softreset`: reset and reboot; all registers return to their defaults.
    pub const SOFT_RESET: u8 = 0xB6;
}

/// ACC_CONF value: `acc_us` = 0, `acc_bwp` = 0b010 (normal filter), `acc_odr` =
/// 0b1010 (400 Hz; ODR = 100 / 2^(8 − val) Hz, datasheet §2.11.11, §2.4.1).
pub const ACC_CONF_400HZ_NORMAL: u8 = 0x2A;
/// ACC_RANGE value for ±16 g (`acc_range` = 0b1100, datasheet §2.11.12).
pub const ACC_RANGE_16G: u8 = 0x0C;
/// FIFO_CONFIG_1 value: `fifo_acc_en` (bit 6) set, `fifo_gyr_en`, `fifo_mag_en`,
/// `fifo_header_en` (bit 4) and `fifo_time_en` (bit 1) cleared: accelerometer
/// frames only, headerless (datasheet §2.11.17).
pub const FIFO_CONFIG_1_ACC_HEADERLESS: u8 = 0x40;

/// Output data rate set by [`Bmi160::init_accel_fifo`], in Hz.
pub const ODR_HZ: u32 = 400;
/// FIFO capacity in bytes (datasheet §2.5).
pub const FIFO_CAPACITY: usize = 1024;
/// Sensitivity at ±16 g in LSB per g (typical, datasheet table 2, S16g).
pub const LSB_PER_G_16G: i32 = 2048;

/// Wait after `acc_set_pmu_mode` normal, in microseconds.
///
/// Datasheet table 24: the CMD register is occupied for at most 3.8 ms, plus
/// 0.3 ms when all sensors were in suspend (footnote 4) → 4.1 ms; rounded up.
pub const ACC_STARTUP_US: u32 = 5_000;

/// Wait after a soft reset, in microseconds.
///
// UNVERIFIED: the datasheet gives no explicit soft-reset time; §2.11.38 only
// says "following a delay" and quotes a 300 µs system start-up time. Bosch's
// reference driver waits 1 ms. 10 ms is a conservative margin.
pub const SOFT_RESET_US: u32 = 10_000;

/// Wait after a register write while every sensor is in suspend or low-power
/// mode, in microseconds. Datasheet §3.2.4 asks for at least 450 µs of
/// interface idle time (§2.2.1 says "~400 µs"); in normal mode only 2 µs is
/// needed, which any I2C master exceeds between transfers. Rounded up.
pub const SUSPEND_WRITE_GAP_US: u32 = 1_000;

/// Convert a raw ±16 g sample to milli-g (integer, truncating toward zero).
///
/// 2048 LSB/g → 1 LSB = 1000/2048 mg = 125/256 mg.
pub const fn raw_to_mg_16g(raw: i16) -> i32 {
    raw as i32 * 125 / 256
}

/// Convert a raw (possibly averaged) ±16 g value to milli-g.
pub fn raw_to_mg_16g_f32(raw: f32) -> f32 {
    raw * (1000.0 / LSB_PER_G_16G as f32)
}

/// Convert a SENSORTIME count to microseconds.
///
/// The counter ticks at 25.6 kHz: datasheet table 11 lists bit 4 toggling
/// every 0.625 ms, so one tick is 0.625 ms / 16 = 39.0625 µs (§2.3.1 rounds
/// this to "39 µs"). The 24-bit counter wraps after about 10 min 55 s.
pub const fn sensortime_to_us(ticks: u32) -> u64 {
    ticks as u64 * 625 / 16
}

/// Accelerometer power mode as reported in PMU_STATUS (datasheet §2.11.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AccPowerMode {
    /// 0b00: suspend.
    Suspend,
    /// 0b01: normal.
    Normal,
    /// 0b10: low power.
    LowPower,
    /// 0b11: not defined by the datasheet.
    Reserved,
}

/// Decoded PMU_STATUS register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PmuStatus(pub u8);

impl PmuStatus {
    /// Accelerometer power mode (`acc_pmu_status`, bits 5:4).
    pub const fn acc(self) -> AccPowerMode {
        match (self.0 >> 4) & 0b11 {
            0b00 => AccPowerMode::Suspend,
            0b01 => AccPowerMode::Normal,
            0b10 => AccPowerMode::LowPower,
            _ => AccPowerMode::Reserved,
        }
    }

    /// Raw gyroscope power mode (`gyr_pmu_status`, bits 3:2; 0 = suspend).
    pub const fn gyr_raw(self) -> u8 {
        (self.0 >> 2) & 0b11
    }
}

/// Decoded ERR_REG register (datasheet §2.11.2). Reading it clears the flags,
/// except `fatal_err`, which only a power-on reset clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ErrReg(pub u8);

impl ErrReg {
    /// Bit 0, `fatal_err`: chip not operable.
    pub const fn fatal(self) -> bool {
        self.0 & 0x01 != 0
    }

    /// Bits 4:1, `error_code` (0 = no error; 6 = ODRs do not match in
    /// headerless mode; see the datasheet for the rest).
    pub const fn error_code(self) -> u8 {
        (self.0 >> 1) & 0x0F
    }

    /// Bit 6, `drop_cmd_err`: a write to CMD was dropped because the previous
    /// command was still executing.
    pub const fn dropped_cmd(self) -> bool {
        self.0 & 0x40 != 0
    }

    /// True when no error flag relevant to this driver is set.
    pub const fn is_ok(self) -> bool {
        !self.fatal() && self.error_code() == 0 && !self.dropped_cmd()
    }
}

/// Errors from the multi-step operations of this driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// The I2C transfer failed.
    Bus(E),
    /// CHIP_ID did not read 0xD1; the payload is the value read.
    WrongChipId(u8),
    /// The accelerometer did not reach normal mode; the payload is PMU_STATUS.
    NotNormalMode(PmuStatus),
    /// ERR_REG reported an error after configuration.
    Device(ErrReg),
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Bus(e)
    }
}

/// Handle for one BMI160 on an I2C bus. Holds only the address; see the crate
/// docs for the bus-ownership model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Bmi160 {
    addr: u8,
}

impl Bmi160 {
    /// Create a handle for the device at `i2c_addr` ([`ADDR_SDO_LOW`] or
    /// [`ADDR_SDO_HIGH`]). No bus traffic.
    pub const fn new(i2c_addr: u8) -> Self {
        Self { addr: i2c_addr }
    }

    /// The I2C address this handle talks to.
    pub const fn address(&self) -> u8 {
        self.addr
    }

    /// Read `buf.len()` bytes starting at register `reg`.
    pub async fn read_regs<I: I2c>(
        &self,
        i2c: &mut I,
        reg: u8,
        buf: &mut [u8],
    ) -> Result<(), I::Error> {
        i2c.write_read(self.addr, &[reg], buf).await
    }

    /// Read one register.
    pub async fn read_reg<I: I2c>(&self, i2c: &mut I, reg: u8) -> Result<u8, I::Error> {
        let mut b = [0u8; 1];
        self.read_regs(i2c, reg, &mut b).await?;
        Ok(b[0])
    }

    /// Write one register.
    pub async fn write_reg<I: I2c>(&self, i2c: &mut I, reg: u8, value: u8) -> Result<(), I::Error> {
        i2c.write(self.addr, &[reg, value]).await
    }

    /// Read CHIP_ID (0x00). A BMI160 returns [`CHIP_ID_VALUE`] (0xD1).
    pub async fn chip_id<I: I2c>(&self, i2c: &mut I) -> Result<u8, I::Error> {
        self.read_reg(i2c, reg::CHIP_ID).await
    }

    /// Read CHIP_ID and fail with [`Error::WrongChipId`] unless it is 0xD1.
    pub async fn check_chip_id<I: I2c>(&self, i2c: &mut I) -> Result<(), Error<I::Error>> {
        match self.chip_id(i2c).await? {
            CHIP_ID_VALUE => Ok(()),
            other => Err(Error::WrongChipId(other)),
        }
    }

    /// Issue a soft reset (CMD ← 0xB6) and wait [`SOFT_RESET_US`].
    ///
    /// Afterwards every register is back at its default: accelerometer and
    /// gyroscope suspended, FIFO disabled.
    pub async fn soft_reset<I: I2c, D: DelayNs>(
        &self,
        i2c: &mut I,
        delay: &mut D,
    ) -> Result<(), I::Error> {
        self.write_reg(i2c, reg::CMD, cmd::SOFT_RESET).await?;
        delay.delay_us(SOFT_RESET_US).await;
        Ok(())
    }

    /// Bring the device into the project configuration.
    ///
    /// Sequence:
    /// 1. soft reset, so the result does not depend on earlier configuration
    ///    (this also leaves the gyroscope suspended, its reset state);
    /// 2. CMD ← 0x11 (accelerometer normal mode), wait [`ACC_STARTUP_US`];
    /// 3. check PMU_STATUS reports the accelerometer in normal mode;
    /// 4. ACC_CONF ← 0x2A (400 Hz, normal filter), ACC_RANGE ← 0x0C (±16 g);
    /// 5. FIFO_CONFIG_1 ← 0x40 (accelerometer only, headerless);
    /// 6. CMD ← 0xB0 (flush the FIFO so it starts with a whole frame);
    /// 7. read ERR_REG and fail if it reports an error.
    ///
    /// FIFO_DOWNS keeps its reset value 0x88 (filtered data, no down-sampling).
    pub async fn init_accel_fifo<I: I2c, D: DelayNs>(
        &self,
        i2c: &mut I,
        delay: &mut D,
    ) -> Result<(), Error<I::Error>> {
        self.soft_reset(i2c, delay).await?;

        self.write_reg(i2c, reg::CMD, cmd::ACC_NORMAL).await?;
        delay.delay_us(ACC_STARTUP_US).await;

        let pmu = self.pmu_status(i2c).await?;
        if pmu.acc() != AccPowerMode::Normal {
            return Err(Error::NotNormalMode(pmu));
        }

        self.write_reg(i2c, reg::ACC_CONF, ACC_CONF_400HZ_NORMAL)
            .await?;
        self.write_reg(i2c, reg::ACC_RANGE, ACC_RANGE_16G).await?;
        self.write_reg(i2c, reg::FIFO_CONFIG_1, FIFO_CONFIG_1_ACC_HEADERLESS)
            .await?;
        self.flush_fifo(i2c).await?;

        let err = self.error_status(i2c).await?;
        if !err.is_ok() {
            return Err(Error::Device(err));
        }
        Ok(())
    }

    /// Put the accelerometer back into suspend (CMD ← 0x10).
    ///
    /// With every sensor suspended the datasheet (§3.2.4) requires 450 µs of
    /// idle time after a register write; this waits [`SUSPEND_WRITE_GAP_US`].
    pub async fn suspend_accel<I: I2c, D: DelayNs>(
        &self,
        i2c: &mut I,
        delay: &mut D,
    ) -> Result<(), I::Error> {
        self.write_reg(i2c, reg::CMD, cmd::ACC_SUSPEND).await?;
        delay.delay_us(SUSPEND_WRITE_GAP_US).await;
        Ok(())
    }

    /// Read PMU_STATUS (0x03).
    pub async fn pmu_status<I: I2c>(&self, i2c: &mut I) -> Result<PmuStatus, I::Error> {
        Ok(PmuStatus(self.read_reg(i2c, reg::PMU_STATUS).await?))
    }

    /// Read ERR_REG (0x02). Reading clears the error flags.
    pub async fn error_status<I: I2c>(&self, i2c: &mut I) -> Result<ErrReg, I::Error> {
        Ok(ErrReg(self.read_reg(i2c, reg::ERR_REG).await?))
    }

    /// Read the latest accelerometer sample from the data registers 0x12–0x17.
    pub async fn read_accel<I: I2c>(&self, i2c: &mut I) -> Result<Accel, I::Error> {
        let mut b = [0u8; Accel::WIRE_LEN];
        self.read_regs(i2c, reg::DATA_ACC_X_LSB, &mut b).await?;
        Ok(Accel::from_le_bytes(b))
    }

    /// Read the FIFO fill level in bytes (FIFO_LENGTH, 11 bits).
    pub async fn fifo_len<I: I2c>(&self, i2c: &mut I) -> Result<u16, I::Error> {
        let mut b = [0u8; 2];
        self.read_regs(i2c, reg::FIFO_LENGTH_0, &mut b).await?;
        Ok(u16::from_le_bytes(b) & 0x07FF)
    }

    /// Burst-read exactly `buf.len()` bytes from FIFO_DATA (0x24).
    ///
    /// Reading more than the FIFO holds returns invalid frames (see
    /// [`crate::fifo`]); a frame read only in part is sent again on the next
    /// read. Prefer [`Bmi160::read_fifo_frames`], which bounds the read by
    /// FIFO_LENGTH. An empty `buf` causes no bus traffic.
    pub async fn read_fifo<I: I2c>(&self, i2c: &mut I, buf: &mut [u8]) -> Result<(), I::Error> {
        if buf.is_empty() {
            return Ok(());
        }
        self.read_regs(i2c, reg::FIFO_DATA, buf).await
    }

    /// Read as many whole 6-byte frames as are both available and fit in `buf`.
    ///
    /// Reads FIFO_LENGTH, rounds down to whole frames and burst-reads that many
    /// bytes into the start of `buf`. Returns the number of bytes read (a
    /// multiple of 6, possibly 0); parse `&buf[..n]` with
    /// [`crate::fifo::frames`].
    pub async fn read_fifo_frames<I: I2c>(
        &self,
        i2c: &mut I,
        buf: &mut [u8],
    ) -> Result<usize, I::Error> {
        let available = usize::from(self.fifo_len(i2c).await?);
        let n = available.min(buf.len()) / Accel::WIRE_LEN * Accel::WIRE_LEN;
        self.read_fifo(i2c, &mut buf[..n]).await?;
        Ok(n)
    }

    /// Read the 24-bit SENSORTIME counter (0x18–0x1A). See [`sensortime_to_us`].
    pub async fn sensortime<I: I2c>(&self, i2c: &mut I) -> Result<u32, I::Error> {
        let mut b = [0u8; 3];
        self.read_regs(i2c, reg::SENSORTIME_0, &mut b).await?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]))
    }

    /// Discard the FIFO contents (CMD ← 0xB0); the FIFO configuration is kept.
    pub async fn flush_fifo<I: I2c>(&self, i2c: &mut I) -> Result<(), I::Error> {
        self.write_reg(i2c, reg::CMD, cmd::FIFO_FLUSH).await
    }
}

#[cfg(test)]
mod tests {
    use embedded_hal::i2c::ErrorKind;
    use embedded_hal_mock::eh1::i2c::{Mock, Transaction};

    use super::*;
    use crate::testutil::{RecordingDelay, block_on};

    const A: u8 = ADDR_SDO_LOW;

    fn init_transactions(pmu: u8, err: u8) -> Vec<Transaction> {
        vec![
            Transaction::write(A, vec![0x7E, 0xB6]),
            Transaction::write(A, vec![0x7E, 0x11]),
            Transaction::write_read(A, vec![0x03], vec![pmu]),
            Transaction::write(A, vec![0x40, 0x2A]),
            Transaction::write(A, vec![0x41, 0x0C]),
            Transaction::write(A, vec![0x47, 0x40]),
            Transaction::write(A, vec![0x7E, 0xB0]),
            Transaction::write_read(A, vec![0x02], vec![err]),
        ]
    }

    #[test]
    fn register_values_match_datasheet_bitfields() {
        // acc_us = 0, acc_bwp = 0b010, acc_odr = 0b1010.
        assert_eq!(ACC_CONF_400HZ_NORMAL, (0b010 << 4) | 0b1010);
        // ODR = 100 / 2^(8 - val) Hz with val = 10.
        assert_eq!(100 * (1 << (0b1010 - 8)), ODR_HZ);
        assert_eq!(ACC_RANGE_16G, 0b1100);
        // fifo_acc_en is bit 6; header (bit 4), gyr (7), mag (5), time (1) off.
        assert_eq!(FIFO_CONFIG_1_ACC_HEADERLESS, 1 << 6);
        assert_eq!(cmd::ACC_NORMAL, 0b0001_0001);
        assert_eq!(cmd::GYR_SUSPEND, 0b0001_0100);
    }

    #[test]
    fn chip_id_reads_register_zero() {
        let mut i2c = Mock::new(&[Transaction::write_read(A, vec![0x00], vec![0xD1])]);
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.chip_id(&mut i2c)), Ok(0xD1));
        i2c.done();
    }

    #[test]
    fn chip_id_uses_the_configured_address() {
        let mut i2c = Mock::new(&[Transaction::write_read(
            ADDR_SDO_HIGH,
            vec![0x00],
            vec![0xD1],
        )]);
        let imu = Bmi160::new(ADDR_SDO_HIGH);
        assert_eq!(imu.address(), 0x69);
        assert_eq!(block_on(imu.check_chip_id(&mut i2c)), Ok(()));
        i2c.done();
    }

    #[test]
    fn check_chip_id_rejects_other_devices() {
        let mut i2c = Mock::new(&[Transaction::write_read(A, vec![0x00], vec![0xD8])]);
        let imu = Bmi160::new(A);
        assert_eq!(
            block_on(imu.check_chip_id(&mut i2c)),
            Err(Error::WrongChipId(0xD8))
        );
        i2c.done();
    }

    #[test]
    fn init_writes_the_documented_sequence_and_waits() {
        // PMU_STATUS 0x10: acc normal, gyro and mag suspended.
        let mut i2c = Mock::new(&init_transactions(0x10, 0x00));
        let mut delay = RecordingDelay::default();
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.init_accel_fifo(&mut i2c, &mut delay)), Ok(()));
        i2c.done();
        // Soft-reset wait, then the accelerometer start-up wait.
        assert_eq!(delay.calls_ns, vec![10_000_000, 5_000_000]);
        // Datasheet table 24: at least 3.8 ms + 0.3 ms after CMD 0x11.
        assert!(delay.calls_ns[1] >= 4_100_000);
    }

    #[test]
    fn init_fails_when_accel_stays_suspended() {
        let all = init_transactions(0x00, 0x00);
        let mut i2c = Mock::new(&all[..3]);
        let mut delay = RecordingDelay::default();
        let imu = Bmi160::new(A);
        assert_eq!(
            block_on(imu.init_accel_fifo(&mut i2c, &mut delay)),
            Err(Error::NotNormalMode(PmuStatus(0x00)))
        );
        i2c.done();
    }

    #[test]
    fn init_reports_err_reg() {
        // error_code = 0b0110 (ODR mismatch in headerless mode) → 0x0C.
        let mut i2c = Mock::new(&init_transactions(0x10, 0x0C));
        let mut delay = RecordingDelay::default();
        let imu = Bmi160::new(A);
        let r = block_on(imu.init_accel_fifo(&mut i2c, &mut delay));
        assert_eq!(r, Err(Error::Device(ErrReg(0x0C))));
        assert_eq!(ErrReg(0x0C).error_code(), 6);
        i2c.done();
    }

    #[test]
    fn init_propagates_bus_errors() {
        let mut i2c =
            Mock::new(&[Transaction::write(A, vec![0x7E, 0xB6]).with_error(ErrorKind::Other)]);
        let mut delay = RecordingDelay::default();
        let imu = Bmi160::new(A);
        assert_eq!(
            block_on(imu.init_accel_fifo(&mut i2c, &mut delay)),
            Err(Error::Bus(ErrorKind::Other))
        );
        assert!(delay.calls_ns.is_empty());
        i2c.done();
    }

    #[test]
    fn read_accel_reads_six_bytes_from_0x12() {
        let mut i2c = Mock::new(&[Transaction::write_read(
            A,
            vec![0x12],
            vec![0x10, 0x00, 0xF0, 0xFF, 0x00, 0x08],
        )]);
        let imu = Bmi160::new(A);
        assert_eq!(
            block_on(imu.read_accel(&mut i2c)),
            Ok(Accel::new(16, -16, 2048))
        );
        i2c.done();
    }

    #[test]
    fn fifo_len_masks_to_eleven_bits() {
        let mut i2c = Mock::new(&[
            Transaction::write_read(A, vec![0x22], vec![0x2A, 0x01]),
            Transaction::write_read(A, vec![0x22], vec![0xFF, 0xFF]),
        ]);
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.fifo_len(&mut i2c)), Ok(0x012A));
        assert_eq!(block_on(imu.fifo_len(&mut i2c)), Ok(0x07FF));
        i2c.done();
    }

    #[test]
    fn read_fifo_bursts_from_0x24() {
        let data: Vec<u8> = (0..12).collect();
        let mut i2c = Mock::new(&[Transaction::write_read(A, vec![0x24], data.clone())]);
        let imu = Bmi160::new(A);
        let mut buf = [0u8; 12];
        assert_eq!(block_on(imu.read_fifo(&mut i2c, &mut buf)), Ok(()));
        assert_eq!(&buf[..], &data[..]);
        i2c.done();
    }

    #[test]
    fn read_fifo_with_empty_buffer_does_not_touch_the_bus() {
        let mut i2c = Mock::new(&[]);
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.read_fifo(&mut i2c, &mut [])), Ok(()));
        i2c.done();
    }

    #[test]
    fn read_fifo_frames_rounds_down_to_whole_frames() {
        // 20 bytes available → 3 whole frames = 18 bytes.
        let data: Vec<u8> = (0..18).collect();
        let mut i2c = Mock::new(&[
            Transaction::write_read(A, vec![0x22], vec![20, 0]),
            Transaction::write_read(A, vec![0x24], data.clone()),
        ]);
        let imu = Bmi160::new(A);
        let mut buf = [0u8; 64];
        assert_eq!(block_on(imu.read_fifo_frames(&mut i2c, &mut buf)), Ok(18));
        assert_eq!(&buf[..18], &data[..]);
        i2c.done();
    }

    #[test]
    fn read_fifo_frames_is_limited_by_the_buffer() {
        // 600 bytes available, 16-byte buffer → 2 frames = 12 bytes.
        let mut i2c = Mock::new(&[
            Transaction::write_read(A, vec![0x22], vec![0x58, 0x02]),
            Transaction::write_read(A, vec![0x24], vec![0xAB; 12]),
        ]);
        let imu = Bmi160::new(A);
        let mut buf = [0u8; 16];
        assert_eq!(block_on(imu.read_fifo_frames(&mut i2c, &mut buf)), Ok(12));
        i2c.done();
    }

    #[test]
    fn read_fifo_frames_with_empty_fifo_only_reads_the_length() {
        let mut i2c = Mock::new(&[Transaction::write_read(A, vec![0x22], vec![5, 0])]);
        let imu = Bmi160::new(A);
        let mut buf = [0u8; 64];
        assert_eq!(block_on(imu.read_fifo_frames(&mut i2c, &mut buf)), Ok(0));
        i2c.done();
    }

    #[test]
    fn sensortime_is_24_bit_little_endian() {
        let mut i2c = Mock::new(&[Transaction::write_read(
            A,
            vec![0x18],
            vec![0x56, 0x34, 0x12],
        )]);
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.sensortime(&mut i2c)), Ok(0x0012_3456));
        i2c.done();
    }

    #[test]
    fn flush_soft_reset_and_suspend_commands() {
        let mut i2c = Mock::new(&[
            Transaction::write(A, vec![0x7E, 0xB0]),
            Transaction::write(A, vec![0x7E, 0xB6]),
            Transaction::write(A, vec![0x7E, 0x10]),
        ]);
        let mut delay = RecordingDelay::default();
        let imu = Bmi160::new(A);
        assert_eq!(block_on(imu.flush_fifo(&mut i2c)), Ok(()));
        assert_eq!(block_on(imu.soft_reset(&mut i2c, &mut delay)), Ok(()));
        assert_eq!(block_on(imu.suspend_accel(&mut i2c, &mut delay)), Ok(()));
        assert_eq!(delay.calls_ns, vec![10_000_000, 1_000_000]);
        i2c.done();
    }

    #[test]
    fn error_and_pmu_status_decode() {
        let mut i2c = Mock::new(&[
            Transaction::write_read(A, vec![0x02], vec![0x41]),
            Transaction::write_read(A, vec![0x03], vec![0x14]),
        ]);
        let imu = Bmi160::new(A);
        let err = block_on(imu.error_status(&mut i2c)).unwrap();
        assert!(err.fatal() && err.dropped_cmd() && !err.is_ok());
        assert_eq!(err.error_code(), 0);
        let pmu = block_on(imu.pmu_status(&mut i2c)).unwrap();
        assert_eq!(pmu.acc(), AccPowerMode::Normal);
        assert_eq!(pmu.gyr_raw(), 0b01);
        assert_eq!(PmuStatus(0x20).acc(), AccPowerMode::LowPower);
        assert_eq!(PmuStatus(0x00).acc(), AccPowerMode::Suspend);
        assert!(ErrReg(0).is_ok());
        i2c.done();
    }

    #[test]
    fn mg_conversion_at_16g() {
        assert_eq!(raw_to_mg_16g(2048), 1000);
        assert_eq!(raw_to_mg_16g(-2048), -1000);
        assert_eq!(raw_to_mg_16g(0), 0);
        assert_eq!(raw_to_mg_16g(i16::MAX), 15_999);
        assert_eq!(raw_to_mg_16g(i16::MIN), -16_000);
        assert!((raw_to_mg_16g_f32(1024.0) - 500.0).abs() < 1e-3);
    }

    #[test]
    fn sensortime_conversion() {
        // 16 ticks = 0.625 ms; one 400 Hz period = 64 ticks = 2.5 ms.
        assert_eq!(sensortime_to_us(16), 625);
        assert_eq!(sensortime_to_us(64), 2_500);
        // Full 24-bit range: about 655.36 s.
        assert_eq!(sensortime_to_us(0x00FF_FFFF), 655_359_960);
    }
}
