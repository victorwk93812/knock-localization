//! Per-sensor bus-health bookkeeping: error counters, the outage state machine
//! and the data-gap tracker used by the soak tests (T3, T4, T13).
//!
//! Everything here is plain arithmetic on counters and microsecond timestamps,
//! so it runs (and is tested) on the host.

use embedded_hal::i2c::ErrorKind;

/// What went wrong with one bus access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ErrorClass {
    /// The device did not acknowledge its address or a data byte.
    Nack,
    /// The transfer did not finish in time (stuck bus).
    Timeout,
    /// CHIP_ID was read but was not the expected value.
    WrongChipId,
    /// Anything else: arbitration loss, bus error, overrun.
    Other,
}

/// Map an `embedded-hal` error kind to an [`ErrorClass`].
///
/// Only [`ErrorKind::NoAcknowledge`] is recognisable this way; a timeout is
/// detected by the bus wrapper and never reaches this function.
pub const fn classify_kind(kind: ErrorKind) -> ErrorClass {
    match kind {
        ErrorKind::NoAcknowledge(_) => ErrorClass::Nack,
        _ => ErrorClass::Other,
    }
}

/// Counters for the bus accesses made to one sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ErrorCounters {
    /// Accesses attempted (successful or not).
    pub reads: u32,
    /// Accesses that ended in a NACK.
    pub nack: u32,
    /// Accesses that timed out.
    pub timeout: u32,
    /// CHIP_ID reads that returned the wrong value.
    pub wrong_chip_id: u32,
    /// Accesses that failed in any other way.
    pub other: u32,
}

impl ErrorCounters {
    /// All counters at zero.
    pub const fn new() -> Self {
        Self {
            reads: 0,
            nack: 0,
            timeout: 0,
            wrong_chip_id: 0,
            other: 0,
        }
    }

    /// Count one successful access.
    pub const fn record_ok(&mut self) {
        self.reads = self.reads.saturating_add(1);
    }

    /// Count one failed access.
    pub const fn record(&mut self, class: ErrorClass) {
        self.reads = self.reads.saturating_add(1);
        let counter = match class {
            ErrorClass::Nack => &mut self.nack,
            ErrorClass::Timeout => &mut self.timeout,
            ErrorClass::WrongChipId => &mut self.wrong_chip_id,
            ErrorClass::Other => &mut self.other,
        };
        *counter = counter.saturating_add(1);
    }

    /// Total number of failed accesses.
    pub const fn errors(&self) -> u32 {
        self.nack
            .saturating_add(self.timeout)
            .saturating_add(self.wrong_chip_id)
            .saturating_add(self.other)
    }

    /// True when no access failed.
    pub const fn is_clean(&self) -> bool {
        self.errors() == 0
    }
}

/// Outage state machine for one sensor.
///
/// ```text
///            fail()                      recover()
/// Healthy ───────────▶ Down { since } ───────────────▶ Healthy
///                        │  ▲
///                        └──┘ fail() (still down, not counted again)
/// ```
///
/// `fail` is called when an access to the sensor fails, `recover` once the
/// sensor answers again *and* has been re-initialised. An outage "ended" when
/// `recover` was called for it, so `outages() - recoveries()` is 1 while the
/// sensor is still down and 0 otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct OutageTracker {
    down_since_us: Option<u64>,
    outages: u32,
    recoveries: u32,
    longest_us: u64,
}

impl OutageTracker {
    /// A healthy sensor with no history.
    pub const fn new() -> Self {
        Self {
            down_since_us: None,
            outages: 0,
            recoveries: 0,
            longest_us: 0,
        }
    }

    /// Record a failed access at `now_us`. Returns true if this starts a new
    /// outage (the sensor was healthy until now).
    pub const fn fail(&mut self, now_us: u64) -> bool {
        if self.down_since_us.is_some() {
            return false;
        }
        self.down_since_us = Some(now_us);
        self.outages = self.outages.saturating_add(1);
        true
    }

    /// Record that the sensor is back (answering and re-initialised) at
    /// `now_us`. Returns the outage duration in microseconds, or `None` if the
    /// sensor was not down.
    pub const fn recover(&mut self, now_us: u64) -> Option<u64> {
        let Some(since) = self.down_since_us else {
            return None;
        };
        self.down_since_us = None;
        self.recoveries = self.recoveries.saturating_add(1);
        let duration = now_us.saturating_sub(since);
        if duration > self.longest_us {
            self.longest_us = duration;
        }
        Some(duration)
    }

    /// True while the sensor is in an outage.
    pub const fn is_down(&self) -> bool {
        self.down_since_us.is_some()
    }

    /// Duration of the outage still open at `now_us`, if any.
    pub const fn open_outage_us(&self, now_us: u64) -> Option<u64> {
        match self.down_since_us {
            Some(since) => Some(now_us.saturating_sub(since)),
            None => None,
        }
    }

    /// Number of outages started.
    pub const fn outages(&self) -> u32 {
        self.outages
    }

    /// Number of outages that ended with a recovery.
    pub const fn recoveries(&self) -> u32 {
        self.recoveries
    }

    /// Longest *finished* outage in microseconds.
    pub const fn longest_outage_us(&self) -> u64 {
        self.longest_us
    }
}

/// Tracks the longest interval between two successive data deliveries of one
/// sensor.
///
/// Call [`GapTracker::data`] each time the sensor delivered data and
/// [`GapTracker::reset`] when the sensor goes into an outage, so the outage
/// itself is not counted as a gap of a *healthy* sensor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct GapTracker {
    last_us: Option<u64>,
    longest_us: u64,
}

impl GapTracker {
    /// No data seen yet.
    pub const fn new() -> Self {
        Self {
            last_us: None,
            longest_us: 0,
        }
    }

    /// Record a data delivery at `now_us`; returns the gap since the previous
    /// one (0 for the first).
    pub const fn data(&mut self, now_us: u64) -> u64 {
        let gap = match self.last_us {
            Some(last) => now_us.saturating_sub(last),
            None => 0,
        };
        self.last_us = Some(now_us);
        if gap > self.longest_us {
            self.longest_us = gap;
        }
        gap
    }

    /// Forget the previous delivery (the next one starts a new interval).
    pub const fn reset(&mut self) {
        self.last_us = None;
    }

    /// Longest gap seen, in microseconds.
    pub const fn longest_us(&self) -> u64 {
        self.longest_us
    }
}

#[cfg(test)]
mod tests {
    use embedded_hal::i2c::NoAcknowledgeSource;

    use super::*;

    #[test]
    fn classify_recognises_nack_only() {
        assert_eq!(
            classify_kind(ErrorKind::NoAcknowledge(NoAcknowledgeSource::Address)),
            ErrorClass::Nack
        );
        assert_eq!(
            classify_kind(ErrorKind::NoAcknowledge(NoAcknowledgeSource::Unknown)),
            ErrorClass::Nack
        );
        assert_eq!(classify_kind(ErrorKind::Bus), ErrorClass::Other);
        assert_eq!(classify_kind(ErrorKind::ArbitrationLoss), ErrorClass::Other);
        assert_eq!(classify_kind(ErrorKind::Other), ErrorClass::Other);
    }

    #[test]
    fn counters_count_each_class_once() {
        let mut c = ErrorCounters::new();
        assert!(c.is_clean());
        c.record_ok();
        c.record_ok();
        assert!(c.is_clean());
        c.record(ErrorClass::Nack);
        c.record(ErrorClass::Timeout);
        c.record(ErrorClass::Timeout);
        c.record(ErrorClass::WrongChipId);
        c.record(ErrorClass::Other);
        assert_eq!(c.reads, 7);
        assert_eq!((c.nack, c.timeout, c.wrong_chip_id, c.other), (1, 2, 1, 1));
        assert_eq!(c.errors(), 5);
        assert!(!c.is_clean());
    }

    #[test]
    fn counters_saturate() {
        let mut c = ErrorCounters {
            reads: u32::MAX,
            nack: u32::MAX,
            ..ErrorCounters::new()
        };
        c.record(ErrorClass::Nack);
        c.record(ErrorClass::Other);
        assert_eq!(c.reads, u32::MAX);
        assert_eq!(c.nack, u32::MAX);
        assert_eq!(c.errors(), u32::MAX);
    }

    #[test]
    fn outage_lifecycle() {
        let mut o = OutageTracker::new();
        assert!(!o.is_down());
        assert_eq!(o.recover(5), None, "recover while healthy is a no-op");
        assert_eq!(o.recoveries(), 0);

        assert!(o.fail(1_000));
        assert!(o.is_down());
        assert!(!o.fail(2_000), "second failure does not start a new outage");
        assert_eq!(o.outages(), 1);
        assert_eq!(o.open_outage_us(4_000), Some(3_000));

        assert_eq!(o.recover(6_000), Some(5_000));
        assert!(!o.is_down());
        assert_eq!(o.open_outage_us(7_000), None);
        assert_eq!((o.outages(), o.recoveries()), (1, 1));
        assert_eq!(o.longest_outage_us(), 5_000);
    }

    #[test]
    fn outage_keeps_the_longest_and_counts_open_ones() {
        let mut o = OutageTracker::new();
        o.fail(0);
        o.recover(100);
        o.fail(1_000);
        o.recover(1_900);
        o.fail(5_000);
        o.recover(5_010);
        assert_eq!(o.longest_outage_us(), 900);
        o.fail(9_000);
        assert_eq!((o.outages(), o.recoveries()), (4, 3));
        assert!(o.is_down());
    }

    #[test]
    fn gap_tracker_measures_between_deliveries() {
        let mut g = GapTracker::new();
        assert_eq!(g.data(1_000), 0);
        assert_eq!(g.data(21_000), 20_000);
        assert_eq!(g.data(81_000), 60_000);
        assert_eq!(g.data(101_000), 20_000);
        assert_eq!(g.longest_us(), 60_000);
    }

    #[test]
    fn gap_tracker_reset_excludes_an_outage() {
        let mut g = GapTracker::new();
        g.data(0);
        g.data(20_000);
        g.reset();
        assert_eq!(g.data(5_000_000), 0);
        assert_eq!(g.data(5_020_000), 20_000);
        assert_eq!(g.longest_us(), 20_000);
    }
}
