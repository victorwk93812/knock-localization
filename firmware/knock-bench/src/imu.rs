//! The nine-sensor BMI160 array: bring-up and round-robin FIFO readout.
//!
//! Sensor numbering (as in the proposal): I0..I7 are the BMI160s at 0x68
//! behind TCA9548A channels 0..7; I8 is the BMI160 at 0x69 directly on the
//! I2C1 trunk; the multiplexer is at 0x70.
//!
//! ```ignore
//! let mut array = imu::bring_up(&mut port).await;   // logs one line per sensor
//! let mut i2c = TimeoutI2c::new(port.bus());
//! array.flush_all(&mut i2c).await;
//! let round = array
//!     .drain_round(&mut i2c, |k, result| match result {
//!         Ok(drain) => for sample in drain.frames { /* ... */ },
//!         Err(fault) => { /* count it */ }
//!     })
//!     .await;
//! if round.aborted.is_some() {
//!     drop(i2c);
//!     port.recover().await; // then build a new bus
//! }
//! ```
//!
//! # Multiplexer policy
//!
//! *The channel of the sensor read last stays selected.* Each visit to I0..I7
//! starts with one write of `1 << k` to the multiplexer, which switches from
//! the previous channel to the new one in a single transfer (the TCA9548A
//! applies the new mask at the STOP condition). The multiplexer is never
//! written to "all off" during normal readout, because that would double the
//! number of mux transfers (16 instead of 8 per round) for no benefit:
//!
//! - I8 answers at 0x69 and the channel sensors at 0x68, so **I8 is read with
//!   whatever channel happens to be selected** (channel 7 in a normal round);
//!   there is no address conflict.
//! - Leaving the channel selected is also what bus recovery needs: the nine
//!   SCL pulses of `I2c1Port::recover` only reach the trunk and the *selected*
//!   channel, and a transfer that hung was by construction on the trunk or on
//!   the channel selected for it.
//!
//! [`ImuArray::park`] switches every channel off; the tests call it when they
//! are done.
//!
//! # Without a multiplexer
//!
//! If nothing answers at 0x70, [`bring_up`] looks for sensors directly on the
//! trunk: 0x68 becomes I0 and 0x69 is I8 as usual; I1..I7 are reported
//! missing. This is the two-sensor set-up for the mounting comparison (T8).
//!
//! # Errors and recovery
//!
//! Every bus access goes through [`TimeoutI2c`]. A NACK or bus error only
//! affects the sensor being read. A **timeout** means the bus state is
//! unknown: [`ImuArray::drain_round`] stops the round at once and reports it
//! in [`Round::aborted`]; the caller must drop the bus, run
//! `I2c1Port::recover` and build a new bus before continuing.

use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::wdg::IndependentWatchdog;
use embassy_stm32::{Peri, peripherals};
use embassy_time::{Delay, Duration, Instant};
use embedded_hal_async::i2c::I2c;
use knock_drivers::bmi160::{self, Bmi160};
use knock_drivers::fifo;
use knock_drivers::health::{self, ErrorClass};
use knock_drivers::tca9548a::{self, Tca9548a};

use crate::board::I2c1Port;
use crate::bus::{BusError, TimeoutI2c};

/// Number of BMI160 sensors in the array.
pub const SENSORS: usize = 9;

/// Index of the sensor on the trunk (I8, address 0x69).
pub const TRUNK_SENSOR: usize = 8;

/// The multiplexer.
pub const MUX: Tca9548a = Tca9548a::new(tca9548a::DEFAULT_ADDRESS);

/// Size of the FIFO read buffer: 170 whole frames, all a 1024-byte FIFO holds.
pub const DRAIN_BUF_LEN: usize = 170 * fifo::FRAME_LEN;

/// The readout period all acquisition tests use.
pub const ROUND_PERIOD: Duration = Duration::from_millis(20);

/// Driver handle for sensor `k` (0x69 for I8, 0x68 for the others).
pub const fn sensor(k: usize) -> Bmi160 {
    if k == TRUNK_SENSOR {
        Bmi160::new(bmi160::ADDR_SDO_HIGH)
    } else {
        Bmi160::new(bmi160::ADDR_SDO_LOW)
    }
}

/// Classify a [`TimeoutI2c`] error for the per-sensor counters.
pub fn classify<E: embedded_hal::i2c::Error>(e: &BusError<E>) -> ErrorClass {
    match e {
        BusError::Timeout => ErrorClass::Timeout,
        BusError::I2c(e) => health::classify_kind(e.kind()),
    }
}

/// Why an operation on one sensor failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// Selecting the sensor's multiplexer channel failed.
    Mux(ErrorClass),
    /// A transfer to the sensor itself failed.
    Bus(ErrorClass),
    /// CHIP_ID was not 0xD1; the payload is the value read.
    WrongChipId(u8),
    /// The accelerometer did not enter normal mode; the payload is PMU_STATUS.
    NotNormalMode(u8),
    /// ERR_REG reported an error after configuration; the payload is ERR_REG.
    Device(u8),
    /// The sensor sits behind the multiplexer and there is no multiplexer.
    NoMux,
}

impl Fault {
    /// The counter this fault belongs in.
    pub const fn class(&self) -> ErrorClass {
        match self {
            Fault::Mux(c) | Fault::Bus(c) => *c,
            Fault::WrongChipId(_) => ErrorClass::WrongChipId,
            Fault::NoMux => ErrorClass::Nack,
            Fault::NotNormalMode(_) | Fault::Device(_) => ErrorClass::Other,
        }
    }

    /// True if a transfer timed out: the bus needs recovery before it is used
    /// again.
    pub const fn is_timeout(&self) -> bool {
        matches!(self.class(), ErrorClass::Timeout)
    }

    fn from_bmi160<E: embedded_hal::i2c::Error>(e: bmi160::Error<BusError<E>>) -> Self {
        match e {
            bmi160::Error::Bus(e) => Fault::Bus(classify(&e)),
            bmi160::Error::WrongChipId(id) => Fault::WrongChipId(id),
            bmi160::Error::NotNormalMode(pmu) => Fault::NotNormalMode(pmu.0),
            bmi160::Error::Device(err) => Fault::Device(err.0),
        }
    }
}

/// One sensor's FIFO contents from one readout.
#[derive(Debug, Clone)]
pub struct Drain<'a> {
    /// The frames read, oldest first.
    pub frames: fifo::Frames<'a>,
    /// FIFO_LENGTH in bytes as read before the burst.
    pub level_bytes: u16,
    /// MCU time right after FIFO_LENGTH was read. The newest frame of the
    /// burst was sampled at most one output period (2.5 ms) before this.
    pub at: Instant,
}

/// Timing and outcome of one [`ImuArray::drain_round`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Round {
    /// When the round started.
    pub started: Instant,
    /// How long the round took (the "loop time").
    pub took: Duration,
    /// Number of sensors whose readout failed.
    pub faults: u8,
    /// `Some(k)` if the round was abandoned at sensor `k` because of a
    /// timeout. Recover the bus before the next round.
    pub aborted: Option<u8>,
}

/// State of the sensor array.
pub struct ImuArray {
    mux_present: bool,
    ready: [bool; SENSORS],
    fault: [Option<Fault>; SENSORS],
    buf: [u8; DRAIN_BUF_LEN],
}

impl ImuArray {
    /// An array with a multiplexer and no sensor marked ready.
    pub const fn new() -> Self {
        Self {
            mux_present: true,
            ready: [false; SENSORS],
            fault: [None; SENSORS],
            buf: [0; DRAIN_BUF_LEN],
        }
    }

    /// False if [`bring_up`] found no multiplexer (trunk-only mode).
    pub const fn mux_present(&self) -> bool {
        self.mux_present
    }

    /// True if sensor `k` is configured and included in the readout.
    pub const fn is_ready(&self, k: usize) -> bool {
        self.ready[k]
    }

    /// Include sensor `k` in, or exclude it from, [`ImuArray::drain_round`].
    pub const fn set_ready(&mut self, k: usize, ready: bool) {
        self.ready[k] = ready;
    }

    /// Number of sensors that are ready.
    pub fn ready_count(&self) -> usize {
        self.ready.iter().filter(|r| **r).count()
    }

    /// Why sensor `k` failed its last [`ImuArray::init_sensor`], if it did.
    pub const fn fault(&self, k: usize) -> Option<Fault> {
        self.fault[k]
    }

    /// Make sensor `k` addressable: select its multiplexer channel.
    ///
    /// No bus traffic for I8, or for I0 in trunk-only mode.
    pub async fn select<I, E>(&self, i2c: &mut I, k: usize) -> Result<(), Fault>
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
    {
        if k == TRUNK_SENSOR {
            return Ok(());
        }
        if !self.mux_present {
            return if k == 0 { Ok(()) } else { Err(Fault::NoMux) };
        }
        MUX.select(i2c, 1 << k)
            .await
            .map_err(|e| Fault::Mux(classify(&e)))
    }

    /// Select sensor `k` and check that it answers with CHIP_ID 0xD1.
    pub async fn probe<I, E>(&self, i2c: &mut I, k: usize) -> Result<(), Fault>
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
    {
        self.select(i2c, k).await?;
        sensor(k)
            .check_chip_id(i2c)
            .await
            .map_err(Fault::from_bmi160)
    }

    /// Probe and configure sensor `k` (soft reset, accelerometer normal mode,
    /// 400 Hz, ±16 g, headerless FIFO; about 16 ms). Marks it ready on
    /// success and not ready on failure.
    pub async fn init_sensor<I, E>(&mut self, i2c: &mut I, k: usize) -> Result<(), Fault>
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
    {
        let result = async {
            self.probe(i2c, k).await?;
            sensor(k)
                .init_accel_fifo(i2c, &mut Delay)
                .await
                .map_err(Fault::from_bmi160)
        }
        .await;
        self.ready[k] = result.is_ok();
        self.fault[k] = result.err();
        result
    }

    /// Read everything in sensor `k`'s FIFO (whole frames only).
    pub async fn drain<I, E>(&mut self, i2c: &mut I, k: usize) -> Result<Drain<'_>, Fault>
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
    {
        self.select(i2c, k).await?;
        let imu = sensor(k);
        let bus = |e: BusError<E>| Fault::Bus(classify(&e));
        let level_bytes = imu.fifo_len(i2c).await.map_err(bus)?;
        let at = Instant::now();
        let n = usize::from(level_bytes).min(DRAIN_BUF_LEN) / fifo::FRAME_LEN * fifo::FRAME_LEN;
        imu.read_fifo(i2c, &mut self.buf[..n]).await.map_err(bus)?;
        Ok(Drain {
            frames: fifo::frames(&self.buf[..n]),
            level_bytes,
            at,
        })
    }

    /// Drain the FIFO of every ready sensor in the order I0..I8 and hand each
    /// result to `sink(k, result)`.
    ///
    /// A sensor that fails is reported to `sink` and the round goes on with
    /// the next one, except after a timeout: then the round stops and
    /// [`Round::aborted`] names the sensor.
    pub async fn drain_round<I, E, F>(&mut self, i2c: &mut I, mut sink: F) -> Round
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
        F: FnMut(usize, Result<Drain<'_>, Fault>),
    {
        let started = Instant::now();
        let mut faults = 0u8;
        let mut aborted = None;
        for k in 0..SENSORS {
            if !self.ready[k] {
                continue;
            }
            let result = self.drain(i2c, k).await;
            let timed_out = matches!(&result, Err(fault) if fault.is_timeout());
            if result.is_err() {
                faults += 1;
            }
            sink(k, result);
            if timed_out {
                aborted = Some(k as u8);
                break;
            }
        }
        Round {
            started,
            took: started.elapsed(),
            faults,
            aborted,
        }
    }

    /// Discard the FIFO contents of every ready sensor, so a measurement
    /// starts with empty FIFOs. Returns the first fault, if any (the remaining
    /// sensors are still flushed unless it was a timeout).
    pub async fn flush_all<I, E>(&mut self, i2c: &mut I) -> Result<(), (usize, Fault)>
    where
        I: I2c<Error = BusError<E>>,
        E: embedded_hal::i2c::Error,
    {
        let mut first = None;
        for k in 0..SENSORS {
            if !self.ready[k] {
                continue;
            }
            let result = match self.select(i2c, k).await {
                Ok(()) => sensor(k)
                    .flush_fifo(i2c)
                    .await
                    .map_err(|e| Fault::Bus(classify(&e))),
                Err(fault) => Err(fault),
            };
            if let Err(fault) = result {
                first.get_or_insert((k, fault));
                if fault.is_timeout() {
                    break;
                }
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// Switch every multiplexer channel off (best effort; errors are ignored).
    pub async fn park<I: I2c>(&self, i2c: &mut I) {
        if self.mux_present {
            let _ = MUX.disable_all(i2c).await;
        }
    }
}

impl Default for ImuArray {
    fn default() -> Self {
        Self::new()
    }
}

/// Human-readable position of sensor `k`, e.g. `"mux ch3, 0x68"`.
pub fn place(array: &ImuArray, k: usize) -> &'static str {
    const BEHIND_MUX: [&str; 8] = [
        "mux ch0, 0x68",
        "mux ch1, 0x68",
        "mux ch2, 0x68",
        "mux ch3, 0x68",
        "mux ch4, 0x68",
        "mux ch5, 0x68",
        "mux ch6, 0x68",
        "mux ch7, 0x68",
    ];
    match k {
        TRUNK_SENSOR => "trunk, 0x69",
        0 if !array.mux_present() => "trunk, 0x68, no mux",
        _ => BEHIND_MUX[k],
    }
}

/// Find and configure every sensor, logging one line per sensor.
///
/// Returns the array with the sensors that answered and accepted the
/// configuration marked ready; [`ImuArray::fault`] tells why the others are
/// not. Never fails: a stuck bus is recovered and the sensor reported
/// missing. The caller decides whether missing sensors fail its test.
pub async fn bring_up(port: &mut I2c1Port) -> ImuArray {
    let mut array = ImuArray::new();

    // The multiplexer first: without it only the trunk can be reached.
    let mux = {
        let mut i2c = TimeoutI2c::new(port.bus());
        crate::scan::probe(&mut i2c, MUX.address()).await
    };
    match mux {
        Ok(true) => crate::note!("mux answers at {:#04x}", MUX.address()),
        Ok(false) => {
            array.mux_present = false;
            crate::note!(
                "no mux at {:#04x}: trunk-only mode, looking for I0 at 0x68 and I8 at 0x69 directly on I2C1",
                MUX.address()
            );
        }
        Err(e) => {
            array.mux_present = false;
            let state = port.recover().await;
            crate::note!(
                "mux probe failed ({:?}), bus recovery: {:?}; continuing in trunk-only mode",
                e,
                state
            );
        }
    }

    for k in 0..SENSORS {
        let result = {
            let mut i2c = TimeoutI2c::new(port.bus());
            array.init_sensor(&mut i2c, k).await
        };
        match result {
            Ok(()) => crate::note!("I{} ({}): ready", k, place(&array, k)),
            Err(Fault::NoMux) => crate::note!("I{}: MISSING (behind the absent mux)", k),
            Err(fault) => {
                crate::note!("I{} ({}): MISSING: {:?}", k, place(&array, k), fault);
                if fault.is_timeout() {
                    let state = port.recover().await;
                    crate::note!("bus recovery: {:?}", state);
                }
            }
        }
    }
    crate::note!("{} of {} sensors ready", array.ready_count(), SENSORS);
    array
}

/// Logic-analyzer probe output on **Arduino D2 = PD14** (push-pull, initially
/// low). The acquisition tests toggle it once per readout round.
///
/// Takes the spare pin handed out as `Board::probe`.
pub fn probe_pin(pin: Peri<'static, peripherals::PD14>) -> Output<'static> {
    Output::new(pin, Level::Low, Speed::Low)
}

/// Create and start the independent watchdog. It cannot be stopped again:
/// call `pet()` at least once per `timeout` for as long as the firmware runs,
/// including while idling after the test.
///
/// Takes the watchdog handed out as `Board::iwdg`.
pub fn start_watchdog(
    iwdg: Peri<'static, peripherals::IWDG>,
    timeout: Duration,
) -> IndependentWatchdog<'static, peripherals::IWDG> {
    let timeout_us = u32::try_from(timeout.as_micros()).unwrap_or(u32::MAX);
    let mut watchdog = IndependentWatchdog::new(iwdg, timeout_us);
    watchdog.unleash();
    watchdog
}

/// RCC_CSR on the STM32L4: RCC base 0x4002_1000 + 0x94 (checked against
/// `stm32-metapac` 21.0.0, `rcc_l4.rs`).
const RCC_CSR: *mut u32 = 0x4002_1094 as *mut u32;
/// RCC_CSR bit 29, IWDGRSTF: the last reset came from the independent watchdog.
const RCC_CSR_IWDGRSTF: u32 = 1 << 29;
/// RCC_CSR bit 23, RMVF: write 1 to clear the reset flags.
const RCC_CSR_RMVF: u32 = 1 << 23;

/// True if this boot was caused by the independent watchdog. Clears the reset
/// flags, so it answers `true` only once per reset; call it once at start-up.
///
/// embassy-stm32 does not expose the PAC without its `unstable-pac` feature,
/// hence the direct register access.
pub fn take_watchdog_reset_flag() -> bool {
    // SAFETY: RCC_CSR is a valid, always-clocked register on this MCU. The
    // read-modify-write only sets RMVF; it runs once at start-up from the
    // single main task, before anything else touches RCC_CSR (embassy only
    // writes it during `init`, which has finished).
    unsafe {
        let csr = RCC_CSR.read_volatile();
        RCC_CSR.write_volatile(csr | RCC_CSR_RMVF);
        csr & RCC_CSR_IWDGRSTF != 0
    }
}
