//! Pin-level I2C bus recovery.
//!
//! When a slave is interrupted in the middle of a read (for example a sensor
//! cable is pulled, or the MCU resets mid-transfer) it can keep SDA low
//! forever, waiting for clock pulses that never come. The standard cure (I2C
//! specification UM10204 §3.1.16) is to clock SCL by hand until the slave
//! releases SDA, then generate a STOP condition.
//!
//! [`recover_bus`] does that on two GPIOs that the caller has configured as
//! **open-drain outputs that can be read back** (`set_high` releases the line,
//! `is_high` reads the actual bus level). It is generic over `embedded-hal`
//! traits so the sequence is unit-tested on the host; the firmware wraps it in
//! `knock_bench::recovery`.

use embedded_hal::digital::{InputPin, OutputPin};
use embedded_hal_async::delay::DelayNs;

/// Number of SCL pulses generated: one full byte plus the ACK bit, enough for
/// any slave to finish the byte it is shifting out.
pub const CLOCK_PULSES: u8 = 9;

/// Half period of the recovery clock in microseconds (50 µs → 10 kHz, slow
/// enough for long wires and weak pull-ups).
pub const DEFAULT_HALF_PERIOD_US: u32 = 50;

/// What [`recover_bus`] observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BusState {
    /// SDA level before the first pulse (`true` = high = released).
    pub sda_high_before: bool,
    /// Number of SCL pulses after which SDA first read high (0 = it was
    /// already high), or `None` if SDA never went high during the pulses.
    pub sda_released_after: Option<u8>,
    /// SCL read high while released, checked before the first pulse. `false`
    /// means something holds the clock low (a short, or a stretching slave).
    pub scl_high: bool,
    /// SDA level after the STOP condition.
    pub sda_high_after: bool,
}

impl BusState {
    /// True when both lines are high at the end, i.e. the bus is idle.
    pub const fn is_idle(&self) -> bool {
        self.scl_high && self.sda_high_after
    }

    /// True when SDA was being held low before recovery started.
    pub const fn was_stuck(&self) -> bool {
        !self.sda_high_before
    }
}

/// Clock out a stuck slave and generate a STOP condition.
///
/// Sequence, with `half_period_us` between edges:
/// 1. release SCL and SDA, sample both;
/// 2. [`CLOCK_PULSES`] times: SCL low, SCL high, sample SDA;
/// 3. STOP: SCL low, SDA low, SCL high, SDA high (SDA rising while SCL is
///    high), sample SDA.
///
/// All nine pulses are always generated: once the slave has released SDA the
/// extra clocks read as NACKed bytes and are harmless, and a fixed sequence is
/// easier to recognise on a logic analyzer.
///
/// Both pins must be open-drain outputs with input read-back. On return both
/// are released (high). The error is the pin error type, which is
/// `Infallible` for most HALs.
pub async fn recover_bus<SCL, SDA, D, E>(
    scl: &mut SCL,
    sda: &mut SDA,
    delay: &mut D,
    half_period_us: u32,
) -> Result<BusState, E>
where
    SCL: OutputPin<Error = E> + InputPin<Error = E>,
    SDA: OutputPin<Error = E> + InputPin<Error = E>,
    D: DelayNs,
{
    scl.set_high()?;
    sda.set_high()?;
    delay.delay_us(half_period_us).await;

    let scl_high = scl.is_high()?;
    let sda_high_before = sda.is_high()?;
    let mut sda_released_after = sda_high_before.then_some(0);

    for pulse in 1..=CLOCK_PULSES {
        scl.set_low()?;
        delay.delay_us(half_period_us).await;
        scl.set_high()?;
        delay.delay_us(half_period_us).await;
        if sda_released_after.is_none() && sda.is_high()? {
            sda_released_after = Some(pulse);
        }
    }

    // STOP condition: SDA rises while SCL is high.
    scl.set_low()?;
    delay.delay_us(half_period_us).await;
    sda.set_low()?;
    delay.delay_us(half_period_us).await;
    scl.set_high()?;
    delay.delay_us(half_period_us).await;
    sda.set_high()?;
    delay.delay_us(half_period_us).await;

    let sda_high_after = sda.is_high()?;

    Ok(BusState {
        sda_high_before,
        sda_released_after,
        scl_high,
        sda_high_after,
    })
}

#[cfg(test)]
mod tests {
    use embedded_hal_mock::eh1::digital::{Mock as PinMock, State, Transaction as PinTransaction};

    use super::*;
    use crate::testutil::{RecordingDelay, block_on};

    /// SCL expectations: release, read, nine pulses, then the STOP edges.
    fn scl_expectations(scl_level: State) -> Vec<PinTransaction> {
        let mut t = vec![
            PinTransaction::set(State::High),
            PinTransaction::get(scl_level),
        ];
        for _ in 0..CLOCK_PULSES {
            t.push(PinTransaction::set(State::Low));
            t.push(PinTransaction::set(State::High));
        }
        t.push(PinTransaction::set(State::Low));
        t.push(PinTransaction::set(State::High));
        t
    }

    /// SDA expectations: release, the given reads, then the STOP edges and the
    /// final read.
    fn sda_expectations(reads: &[State], after: State) -> Vec<PinTransaction> {
        let mut t = vec![PinTransaction::set(State::High)];
        t.extend(reads.iter().map(|s| PinTransaction::get(*s)));
        t.push(PinTransaction::set(State::Low));
        t.push(PinTransaction::set(State::High));
        t.push(PinTransaction::get(after));
        t
    }

    #[test]
    fn idle_bus_still_gets_nine_pulses_and_a_stop() {
        let mut scl = PinMock::new(&scl_expectations(State::High));
        // SDA already high: sampled once, never again during the pulses.
        let mut sda = PinMock::new(&sda_expectations(&[State::High], State::High));
        let mut delay = RecordingDelay::default();

        let s = block_on(recover_bus(&mut scl, &mut sda, &mut delay, 50)).unwrap();
        assert_eq!(
            s,
            BusState {
                sda_high_before: true,
                sda_released_after: Some(0),
                scl_high: true,
                sda_high_after: true,
            }
        );
        assert!(s.is_idle());
        assert!(!s.was_stuck());
        // 1 settle + 9 × 2 pulse edges + 4 STOP edges, 50 µs each.
        assert_eq!(delay.calls_ns.len(), 23);
        assert!(delay.calls_ns.iter().all(|&ns| ns == 50_000));
        scl.done();
        sda.done();
    }

    #[test]
    fn stuck_slave_releases_sda_after_three_pulses() {
        let mut scl = PinMock::new(&scl_expectations(State::High));
        // Low before; low after pulses 1 and 2; high after pulse 3.
        let reads = [State::Low, State::Low, State::Low, State::High];
        let mut sda = PinMock::new(&sda_expectations(&reads, State::High));
        let mut delay = RecordingDelay::default();

        let s = block_on(recover_bus(&mut scl, &mut sda, &mut delay, 50)).unwrap();
        assert_eq!(s.sda_released_after, Some(3));
        assert!(s.was_stuck());
        assert!(s.is_idle());
        scl.done();
        sda.done();
    }

    #[test]
    fn sda_held_low_throughout_is_reported() {
        let mut scl = PinMock::new(&scl_expectations(State::High));
        let reads = [State::Low; 10];
        let mut sda = PinMock::new(&sda_expectations(&reads, State::Low));
        let mut delay = RecordingDelay::default();

        let s = block_on(recover_bus(&mut scl, &mut sda, &mut delay, 50)).unwrap();
        assert_eq!(s.sda_released_after, None);
        assert!(!s.sda_high_after);
        assert!(!s.is_idle());
        scl.done();
        sda.done();
    }

    #[test]
    fn scl_held_low_is_reported() {
        let mut scl = PinMock::new(&scl_expectations(State::Low));
        let mut sda = PinMock::new(&sda_expectations(&[State::High], State::High));
        let mut delay = RecordingDelay::default();

        let s = block_on(recover_bus(&mut scl, &mut sda, &mut delay, 10)).unwrap();
        assert!(!s.scl_high);
        assert!(!s.is_idle());
        assert_eq!(delay.total_ns(), 23 * 10_000);
        scl.done();
        sda.done();
    }
}
