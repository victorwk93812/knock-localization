//! Output-data-rate estimation and FIFO overflow detection.
//!
//! Headerless FIFO frames carry no timestamps, so the only rate information is
//! "how many frames arrived between two bursts". [`RateMeter`] turns that into
//! a rate; [`OverflowDetector`] decides from the fill level and the time since
//! the previous drain whether frames were lost.

/// Frames per second for `frames` frames in `elapsed_us` microseconds.
/// `None` if no time has passed.
pub fn rate_hz(frames: u64, elapsed_us: u64) -> Option<f32> {
    if elapsed_us == 0 {
        return None;
    }
    Some((frames as f64 * 1e6 / elapsed_us as f64) as f32)
}

/// Number of frames a sensor running at `odr_hz` produces in `elapsed_us`
/// microseconds (rounded down).
pub const fn expected_frames(elapsed_us: u64, odr_hz: u32) -> u64 {
    elapsed_us.saturating_mul(odr_hz as u64) / 1_000_000
}

/// True if `value` is within `percent` % of `nominal` (inclusive).
pub fn within_percent(value: f32, nominal: f32, percent: f32) -> bool {
    libm::fabsf(value - nominal) <= libm::fabsf(nominal) * percent / 100.0
}

/// Deviation of `value` from `nominal` in percent (signed).
pub fn deviation_percent(value: f32, nominal: f32) -> f32 {
    (value - nominal) / nominal * 100.0
}

/// Measures a sensor's frame rate from FIFO bursts.
///
/// The first burst only sets the time origin: its frames were produced before
/// the measurement started and are not counted. Every later burst adds its
/// frames, and the rate is `frames / (last burst time − first burst time)`.
///
/// The burst time is when FIFO_LENGTH was read, so the frame count is off by
/// at most one frame at each end; over 10 s at 400 Hz that is 0.05 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct RateMeter {
    first_us: Option<u64>,
    last_us: u64,
    frames: u64,
}

impl RateMeter {
    /// A meter with no bursts yet.
    pub const fn new() -> Self {
        Self {
            first_us: None,
            last_us: 0,
            frames: 0,
        }
    }

    /// Record a burst of `frames` frames whose FIFO level was read at `at_us`.
    pub const fn burst(&mut self, at_us: u64, frames: u32) {
        if self.first_us.is_none() {
            self.first_us = Some(at_us);
        } else {
            self.frames += frames as u64;
        }
        self.last_us = at_us;
    }

    /// Frames counted (excluding the first burst).
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// Time between the first and the last burst, in microseconds.
    pub const fn span_us(&self) -> u64 {
        match self.first_us {
            Some(first) => self.last_us.saturating_sub(first),
            None => 0,
        }
    }

    /// Estimated frame rate in Hz; `None` before the second burst.
    pub fn rate_hz(&self) -> Option<f32> {
        rate_hz(self.frames, self.span_us())
    }
}

/// FIFO capacity in whole 6-byte frames (1024 / 6).
pub const FIFO_CAPACITY_FRAMES: u16 = 170;

/// Fill level in bytes at or above which the FIFO is treated as having
/// overflowed.
///
/// The BMI160 raises "FIFO full" when less than two maximum-size frames of
/// space are left (datasheet §2.6.12) and then overwrites the oldest data
/// (§2.5.2.1). In headerless mode there is no skip frame to announce the
/// loss, so a level this close to the 1024-byte capacity is taken as proof.
/// 1000 bytes = 166 frames = 415 ms of data at 400 Hz.
pub const FULL_LEVEL_BYTES: u16 = 1000;

/// Detects FIFO overflow for one sensor and remembers the highest fill level.
///
/// Two independent signs are used:
///
/// 1. the fill level read before a drain is at least [`FULL_LEVEL_BYTES`];
/// 2. the time since the previous drain is longer than the FIFO can hold at
///    the output data rate ("frame-count jump": more frames were produced
///    than could have been stored).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct OverflowDetector {
    odr_hz: u32,
    last_us: Option<u64>,
    max_level_frames: u16,
    overflows: u32,
}

impl OverflowDetector {
    /// A detector for a sensor running at `odr_hz`.
    pub const fn new(odr_hz: u32) -> Self {
        Self {
            odr_hz,
            last_us: None,
            max_level_frames: 0,
            overflows: 0,
        }
    }

    /// Record a drain: `level_bytes` is FIFO_LENGTH as read at `at_us`, just
    /// before the FIFO was emptied. Returns true if this drain shows an
    /// overflow.
    pub const fn observe(&mut self, at_us: u64, level_bytes: u16) -> bool {
        let level_frames = level_bytes / 6;
        if level_frames > self.max_level_frames {
            self.max_level_frames = level_frames;
        }
        let too_long = match self.last_us {
            Some(last) => {
                expected_frames(at_us.saturating_sub(last), self.odr_hz)
                    > FIFO_CAPACITY_FRAMES as u64
            }
            None => false,
        };
        self.last_us = Some(at_us);
        let overflow = level_bytes >= FULL_LEVEL_BYTES || too_long;
        if overflow {
            self.overflows = self.overflows.saturating_add(1);
        }
        overflow
    }

    /// Forget the previous drain time, after the FIFO was flushed or the
    /// sensor re-initialised (the next drain cannot be judged by its gap).
    pub const fn restart(&mut self) {
        self.last_us = None;
    }

    /// Highest fill level seen, in frames.
    pub const fn max_level_frames(&self) -> u16 {
        self.max_level_frames
    }

    /// Number of drains that showed an overflow.
    pub const fn overflows(&self) -> u32 {
        self.overflows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_and_expected_frames() {
        assert_eq!(rate_hz(10, 0), None);
        assert!((rate_hz(4000, 10_000_000).unwrap() - 400.0).abs() < 1e-3);
        assert_eq!(expected_frames(10_000_000, 400), 4000);
        assert_eq!(expected_frames(2_499, 400), 0);
        assert_eq!(expected_frames(2_500, 400), 1);
        // 30 minutes does not overflow.
        assert_eq!(expected_frames(1_800_000_000, 400), 720_000);
    }

    #[test]
    fn percent_window_is_inclusive() {
        assert!(within_percent(400.0, 400.0, 2.0));
        assert!(within_percent(408.0, 400.0, 2.0));
        assert!(within_percent(392.0, 400.0, 2.0));
        assert!(!within_percent(408.5, 400.0, 2.0));
        assert!(!within_percent(391.0, 400.0, 2.0));
        assert!((deviation_percent(404.0, 400.0) - 1.0).abs() < 1e-4);
        assert!((deviation_percent(396.0, 400.0) + 1.0).abs() < 1e-4);
    }

    #[test]
    fn rate_meter_ignores_the_first_burst() {
        let mut m = RateMeter::new();
        assert_eq!(m.rate_hz(), None);
        m.burst(1_000_000, 72); // backlog from before the measurement
        assert_eq!(m.rate_hz(), None);
        for i in 1..=500u64 {
            m.burst(1_000_000 + i * 20_000, 8);
        }
        assert_eq!(m.frames(), 4000);
        assert_eq!(m.span_us(), 10_000_000);
        assert!((m.rate_hz().unwrap() - 400.0).abs() < 1e-3);
    }

    #[test]
    fn rate_meter_sees_a_fast_oscillator() {
        let mut m = RateMeter::new();
        m.burst(0, 0);
        // 1 % fast: 4040 frames in 10 s.
        m.burst(10_000_000, 4040);
        let r = m.rate_hz().unwrap();
        assert!((r - 404.0).abs() < 1e-3);
        assert!(within_percent(r, 400.0, 2.0));
        assert!(!within_percent(r, 400.0, 0.5));
    }

    #[test]
    fn normal_drains_are_not_overflows() {
        let mut d = OverflowDetector::new(400);
        for i in 0..100u64 {
            assert!(!d.observe(i * 20_000, 48));
        }
        assert_eq!(d.max_level_frames(), 8);
        assert_eq!(d.overflows(), 0);
    }

    #[test]
    fn a_full_fifo_is_an_overflow() {
        let mut d = OverflowDetector::new(400);
        assert!(!d.observe(0, 996));
        assert!(d.observe(20_000, 1000));
        assert!(d.observe(40_000, 1020));
        assert_eq!(d.overflows(), 2);
        assert_eq!(d.max_level_frames(), 170);
    }

    #[test]
    fn a_gap_longer_than_the_fifo_is_an_overflow() {
        let mut d = OverflowDetector::new(400);
        assert!(!d.observe(0, 48));
        // 425 ms = exactly 170 frames: still fits.
        assert!(!d.observe(425_000, 900));
        // 430 ms = 172 frames: cannot have fitted, whatever the level says.
        assert!(d.observe(855_000, 900));
        assert_eq!(d.overflows(), 1);
    }

    #[test]
    fn restart_forgives_the_next_gap() {
        let mut d = OverflowDetector::new(400);
        d.observe(0, 48);
        d.restart();
        assert!(!d.observe(10_000_000, 48));
        assert_eq!(d.overflows(), 0);
    }
}
