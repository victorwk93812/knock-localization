//! Knock trigger and window extraction for FIFO-fed accelerometer streams.
//!
//! The pieces, in the order the capture loop uses them:
//!
//! - [`SampleRing`]: per-sensor ring buffer addressed by a running sample
//!   number (`seq`), with a time anchor so sample numbers and MCU time can be
//!   converted into each other;
//! - [`Baseline`]: slow gravity estimate; `|a − baseline|` is the *dynamic*
//!   acceleration the trigger compares against a threshold;
//! - [`Earliest`]: picks the first threshold crossing among the sensors;
//! - [`Refractory`]: hold-off after a trigger;
//! - [`extract_window`] / [`peak_dynamic`]: cut the −pre…+post window out of a
//!   ring and compute the peak dynamic amplitude A_i.
//!
//! # Sample times
//!
//! Headerless FIFO frames have no timestamps. After each burst the caller
//! stamps the ring with the MCU time at which FIFO_LENGTH was read
//! ([`SampleRing::stamp`]); the newest frame of the burst is taken to have that
//! time and older samples are spaced one output period apart
//! ([`SAMPLE_PERIOD_US`]). The newest frame is really up to one period older
//! than the stamp, and the nine sensors are not synchronised to each other, so
//! windows of different sensors are aligned to about ±1 sample (±2.5 ms).

use crate::Accel;

/// Sample period at the project output data rate of 400 Hz, in microseconds.
pub const SAMPLE_PERIOD_US: u64 = 2_500;

/// Ring buffer holding the newest `N` samples of one sensor.
///
/// Samples are numbered from 0 in arrival order (`seq`); the number of a
/// sample never changes, so a window can be chosen by `seq` first and read
/// later, once the post-trigger samples have arrived.
#[derive(Debug, Clone)]
pub struct SampleRing<const N: usize> {
    buf: [Accel; N],
    total: u64,
    anchor_us: u64,
}

impl<const N: usize> Default for SampleRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> SampleRing<N> {
    /// An empty ring.
    pub const fn new() -> Self {
        Self {
            buf: [Accel::new(0, 0, 0); N],
            total: 0,
            anchor_us: 0,
        }
    }

    /// Append a sample; returns its `seq`.
    pub const fn push(&mut self, sample: Accel) -> u64 {
        let seq = self.total;
        self.buf[(seq % N as u64) as usize] = sample;
        self.total += 1;
        seq
    }

    /// Number of samples pushed so far (= `seq` of the next sample).
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// `seq` of the oldest sample still held.
    pub const fn oldest_seq(&self) -> u64 {
        self.total.saturating_sub(N as u64)
    }

    /// The sample with number `seq`, if it has arrived and is still held.
    pub const fn get(&self, seq: u64) -> Option<Accel> {
        if seq >= self.total || seq < self.oldest_seq() {
            return None;
        }
        Some(self.buf[(seq % N as u64) as usize])
    }

    /// Declare that the newest sample was taken at `at_us` (MCU time).
    /// Call after pushing each burst.
    pub const fn stamp(&mut self, at_us: u64) {
        self.anchor_us = at_us;
    }

    /// Estimated time of sample `seq` (past or future) from the latest stamp.
    /// `None` while the ring is empty.
    pub const fn time_of(&self, seq: u64) -> Option<u64> {
        if self.total == 0 {
            return None;
        }
        let newest = self.total - 1;
        Some(if seq <= newest {
            self.anchor_us
                .saturating_sub((newest - seq) * SAMPLE_PERIOD_US)
        } else {
            self.anchor_us + (seq - newest) * SAMPLE_PERIOD_US
        })
    }

    /// The `seq` whose estimated time is nearest to `t_us` (it may not have
    /// arrived yet). `None` while the ring is empty or if `t_us` lies before
    /// sample 0.
    pub const fn seq_at(&self, t_us: u64) -> Option<u64> {
        if self.total == 0 {
            return None;
        }
        let newest = self.total - 1;
        let half = SAMPLE_PERIOD_US / 2;
        if t_us <= self.anchor_us {
            let steps = (self.anchor_us - t_us + half) / SAMPLE_PERIOD_US;
            newest.checked_sub(steps)
        } else {
            let steps = (t_us - self.anchor_us + half) / SAMPLE_PERIOD_US;
            Some(newest + steps)
        }
    }
}

/// Weight of a new sample in the [`Baseline`] average (time constant 64
/// samples = 160 ms at 400 Hz).
pub const BASELINE_ALPHA: f32 = 1.0 / 64.0;

/// After this many consecutive samples above the gate (1 s at 400 Hz) the
/// baseline is assumed to be wrong (the plate was tilted or moved) and is
/// restarted from the current sample.
pub const BASELINE_REPRIME_AFTER: u16 = 400;

/// Slowly tracking gravity estimate for one sensor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    mean: [f32; 3],
    primed: bool,
    rejected: u16,
}

impl Default for Baseline {
    fn default() -> Self {
        Self::new()
    }
}

impl Baseline {
    /// A baseline with no data; the first sample sets it.
    pub const fn new() -> Self {
        Self {
            mean: [0.0; 3],
            primed: false,
            rejected: 0,
        }
    }

    /// The current estimate `[x, y, z]` in LSB, once a sample has been seen.
    pub const fn value(&self) -> Option<[f32; 3]> {
        if self.primed { Some(self.mean) } else { None }
    }

    /// Dynamic acceleration `|a − baseline|` in LSB (0 before the first
    /// sample).
    pub fn dynamic(&self, a: Accel) -> f32 {
        if !self.primed {
            return 0.0;
        }
        deviation(a, self.mean)
    }

    /// Feed one sample: returns its dynamic acceleration (measured against
    /// the baseline *before* this sample) and then lets the baseline follow
    /// the sample, unless the dynamic acceleration exceeds `gate_lsb` (a knock
    /// must not drag the gravity estimate along).
    pub fn track(&mut self, a: Accel, gate_lsb: f32) -> f32 {
        let v = [f32::from(a.x), f32::from(a.y), f32::from(a.z)];
        if !self.primed {
            self.mean = v;
            self.primed = true;
            return 0.0;
        }
        let d = deviation(a, self.mean);
        if d <= gate_lsb {
            self.rejected = 0;
            for (m, s) in self.mean.iter_mut().zip(v) {
                *m += BASELINE_ALPHA * (s - *m);
            }
        } else {
            self.rejected = self.rejected.saturating_add(1);
            if self.rejected >= BASELINE_REPRIME_AFTER {
                self.mean = v;
                self.rejected = 0;
            }
        }
        d
    }
}

/// `|a − reference|` in LSB.
fn deviation(a: Accel, reference: [f32; 3]) -> f32 {
    crate::stats::magnitude(
        f32::from(a.x) - reference[0],
        f32::from(a.y) - reference[1],
        f32::from(a.z) - reference[2],
    )
}

/// Keeps the earliest of the threshold crossings offered to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Earliest {
    best: Option<(u64, u8)>,
}

impl Earliest {
    /// No crossing yet.
    pub const fn new() -> Self {
        Self { best: None }
    }

    /// Offer a crossing at `t_us` on `sensor`. Ties keep the first offer.
    pub const fn offer(&mut self, t_us: u64, sensor: u8) {
        match self.best {
            Some((best, _)) if best <= t_us => {}
            _ => self.best = Some((t_us, sensor)),
        }
    }

    /// The earliest crossing as `(time in µs, sensor)`.
    pub const fn get(&self) -> Option<(u64, u8)> {
        self.best
    }
}

/// Hold-off after a trigger: new triggers are ignored until it has expired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Refractory {
    until_us: u64,
}

impl Refractory {
    /// Ready immediately.
    pub const fn new() -> Self {
        Self { until_us: 0 }
    }

    /// Start a hold-off of `duration_us` counted from `trigger_us`.
    pub const fn start(&mut self, trigger_us: u64, duration_us: u64) {
        self.until_us = trigger_us.saturating_add(duration_us);
    }

    /// True if an event at `t_us` may trigger.
    pub const fn ready(&self, t_us: u64) -> bool {
        t_us >= self.until_us
    }
}

/// Why a window could not be extracted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum WindowError {
    /// The window reaches past the newest sample: wait for more data.
    NotYet,
    /// The window starts before the oldest sample still held (or before
    /// sample 0): the data is gone.
    TooOld,
}

/// Copy the window `center − pre ..` (`out.len()` samples) from `ring` into
/// `out`. `out[pre]` is the sample numbered `center`.
pub fn extract_window<const N: usize>(
    ring: &SampleRing<N>,
    center: u64,
    pre: usize,
    out: &mut [Accel],
) -> Result<(), WindowError> {
    let start = center.checked_sub(pre as u64).ok_or(WindowError::TooOld)?;
    if start < ring.oldest_seq() {
        return Err(WindowError::TooOld);
    }
    if start + out.len() as u64 > ring.total() {
        return Err(WindowError::NotYet);
    }
    for (i, slot) in out.iter_mut().enumerate() {
        // In range by the two checks above.
        *slot = ring.get(start + i as u64).ok_or(WindowError::TooOld)?;
    }
    Ok(())
}

/// True once every sample of the window `center − pre .. center + post`
/// (inclusive of `center`, `post` samples from `center` on) has arrived.
pub const fn window_complete<const N: usize>(
    ring: &SampleRing<N>,
    center: u64,
    post: usize,
) -> bool {
    ring.total() >= center + post as u64
}

/// Mean of `samples` per axis, in LSB. `[0, 0, 0]` for an empty slice.
pub fn mean_vector(samples: &[Accel]) -> [f32; 3] {
    if samples.is_empty() {
        return [0.0; 3];
    }
    let mut sum = [0i32; 3];
    for s in samples {
        sum[0] += i32::from(s.x);
        sum[1] += i32::from(s.y);
        sum[2] += i32::from(s.z);
    }
    let n = samples.len() as f32;
    [sum[0] as f32 / n, sum[1] as f32 / n, sum[2] as f32 / n]
}

/// Peak dynamic acceleration of a knock window, in LSB:
/// `A = max_t |a(t) − g|`, with the gravity baseline `g` taken as the mean of
/// the first `pre` samples (the pre-trigger part). 0 for an empty window.
pub fn peak_dynamic(window: &[Accel], pre: usize) -> f32 {
    let pre = pre.min(window.len());
    let g = mean_vector(&window[..pre]);
    window.iter().map(|s| deviation(*s, g)).fold(0.0, f32::max)
}

/// Index of the largest value; `None` for an empty slice or if every value is
/// zero or negative. Ties keep the first.
pub fn argmax(values: &[f32]) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, &v) in values.iter().enumerate() {
        if v > 0.0 && best.is_none_or(|(_, b)| v > b) {
            best = Some((i, v));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REST: Accel = Accel::new(0, 0, 2048);

    fn ring_with<const N: usize>(n: u64) -> SampleRing<N> {
        let mut r = SampleRing::new();
        for i in 0..n {
            r.push(Accel::new(i as i16, 0, 2048));
        }
        r
    }

    #[test]
    fn ring_numbers_samples_and_forgets_old_ones() {
        let mut r: SampleRing<8> = SampleRing::new();
        assert_eq!(r.total(), 0);
        assert_eq!(r.get(0), None);
        for i in 0..5 {
            assert_eq!(r.push(Accel::new(i, 0, 0)), i as u64);
        }
        assert_eq!(r.oldest_seq(), 0);
        assert_eq!(r.get(4), Some(Accel::new(4, 0, 0)));
        assert_eq!(r.get(5), None);
        for i in 5..20 {
            r.push(Accel::new(i, 0, 0));
        }
        assert_eq!(r.total(), 20);
        assert_eq!(r.oldest_seq(), 12);
        assert_eq!(r.get(11), None);
        assert_eq!(r.get(12), Some(Accel::new(12, 0, 0)));
        assert_eq!(r.get(19), Some(Accel::new(19, 0, 0)));
    }

    #[test]
    fn ring_time_and_seq_conversions_round_trip() {
        let mut r: SampleRing<64> = ring_with(40);
        assert_eq!(SampleRing::<4>::new().time_of(0), None);
        assert_eq!(SampleRing::<4>::new().seq_at(100), None);
        r.stamp(1_000_000); // sample 39
        assert_eq!(r.time_of(39), Some(1_000_000));
        assert_eq!(r.time_of(35), Some(990_000));
        assert_eq!(r.time_of(41), Some(1_005_000));
        for seq in [0, 10, 39, 45] {
            assert_eq!(r.seq_at(r.time_of(seq).unwrap()), Some(seq));
        }
        // Nearest sample: 1.2 ms before sample 35 is still 35, 1.3 ms is 34.
        assert_eq!(r.seq_at(990_000 - 1_200), Some(35));
        assert_eq!(r.seq_at(990_000 - 1_300), Some(34));
        // Before sample 0.
        assert_eq!(r.seq_at(1_000_000 - 40 * 2_500), None);
    }

    #[test]
    fn restamping_keeps_sample_numbers_meaningful() {
        let mut r: SampleRing<64> = ring_with(8);
        r.stamp(100_000);
        let knock = r.seq_at(95_000).unwrap();
        assert_eq!(knock, 5);
        for _ in 0..8 {
            r.push(REST);
        }
        r.stamp(120_000);
        assert_eq!(r.time_of(knock), Some(95_000));
    }

    #[test]
    fn baseline_follows_rest_and_ignores_a_knock() {
        let mut b = Baseline::new();
        assert_eq!(b.value(), None);
        assert_eq!(b.dynamic(REST), 0.0);
        assert_eq!(b.track(REST, 600.0), 0.0);
        for _ in 0..100 {
            assert!(b.track(Accel::new(2, -2, 2050), 600.0) < 10.0);
        }
        let before = b.value().unwrap();
        let d = b.track(Accel::new(0, 0, 2048 + 4000), 600.0);
        assert!((d - 4000.0).abs() < 10.0);
        assert_eq!(b.value().unwrap(), before, "knock sample must not move it");
        assert!(b.dynamic(REST) < 10.0);
    }

    #[test]
    fn baseline_converges_to_a_small_shift() {
        let mut b = Baseline::new();
        b.track(REST, 600.0);
        for _ in 0..1000 {
            b.track(Accel::new(100, 0, 2048), 600.0);
        }
        assert!((b.value().unwrap()[0] - 100.0).abs() < 0.5);
    }

    #[test]
    fn baseline_reprimes_after_a_lasting_change() {
        let mut b = Baseline::new();
        b.track(REST, 600.0);
        let tilted = Accel::new(2048, 0, 0);
        for _ in 0..BASELINE_REPRIME_AFTER - 1 {
            assert!(b.track(tilted, 600.0) > 600.0);
        }
        b.track(tilted, 600.0);
        assert_eq!(b.value(), Some([2048.0, 0.0, 0.0]));
        assert_eq!(b.dynamic(tilted), 0.0);
    }

    #[test]
    fn earliest_crossing_wins() {
        let mut e = Earliest::new();
        assert_eq!(e.get(), None);
        e.offer(500, 3);
        e.offer(700, 1);
        e.offer(300, 8);
        e.offer(300, 2);
        assert_eq!(e.get(), Some((300, 8)));
    }

    #[test]
    fn refractory_blocks_until_expiry() {
        let mut r = Refractory::new();
        assert!(r.ready(0));
        r.start(1_000_000, 300_000);
        assert!(!r.ready(1_000_000));
        assert!(!r.ready(1_299_999));
        assert!(r.ready(1_300_000));
    }

    #[test]
    fn window_extraction() {
        let r: SampleRing<64> = ring_with(40);
        let mut w = [Accel::default(); 20];
        assert_eq!(extract_window(&r, 10, 4, &mut w), Ok(()));
        assert_eq!(w[0].x, 6);
        assert_eq!(w[4].x, 10, "out[pre] is the centre sample");
        assert_eq!(w[19].x, 25);

        // Needs samples 21..=40 but only 0..=39 exist.
        assert!(!window_complete(&r, 25, 16));
        assert_eq!(extract_window(&r, 25, 4, &mut w), Err(WindowError::NotYet));
        assert!(window_complete(&r, 24, 16));
        assert_eq!(extract_window(&r, 24, 4, &mut w), Ok(()));

        // Centre too close to the start of the stream.
        assert_eq!(extract_window(&r, 3, 4, &mut w), Err(WindowError::TooOld));
    }

    #[test]
    fn window_lost_when_overwritten() {
        let r: SampleRing<32> = ring_with(100);
        let mut w = [Accel::default(); 20];
        assert_eq!(r.oldest_seq(), 68);
        assert_eq!(extract_window(&r, 71, 4, &mut w), Err(WindowError::TooOld));
        assert_eq!(extract_window(&r, 72, 4, &mut w), Ok(()));
        assert_eq!(w[0].x, 68);
    }

    #[test]
    fn peak_uses_the_pre_trigger_baseline() {
        let mut w = [Accel::new(10, -20, 2048); 20];
        w[5] = Accel::new(10, -20, 2048 + 3000);
        w[6] = Accel::new(10 + 300, -20 - 400, 2048); // 500 off the baseline
        assert!((peak_dynamic(&w, 4) - 3000.0).abs() < 1e-3);
        w[5] = w[0];
        assert!((peak_dynamic(&w, 4) - 500.0).abs() < 1e-3);
        assert_eq!(peak_dynamic(&[], 4), 0.0);
        assert_eq!(mean_vector(&[]), [0.0; 3]);
        assert_eq!(mean_vector(&w[..4]), [10.0, -20.0, 2048.0]);
    }

    #[test]
    fn argmax_skips_absent_sensors() {
        assert_eq!(argmax(&[]), None);
        assert_eq!(argmax(&[0.0, 0.0]), None);
        assert_eq!(argmax(&[0.0, 3.0, 7.0, 7.0, 1.0]), Some(2));
    }

    /// End to end: a knock in a simulated 400 Hz stream delivered in 8-frame
    /// bursts is found, windowed and measured.
    #[test]
    fn simulated_knock_capture() {
        const THRESHOLD: f32 = 614.0; // 300 mg
        let mut ring: SampleRing<128> = SampleRing::new();
        let mut baseline = Baseline::new();
        let mut crossing: Option<u64> = None;
        let knock_at = 203u64;

        for burst in 0..40u64 {
            for i in 0..8 {
                let n = burst * 8 + i;
                let a = match n.checked_sub(knock_at) {
                    Some(0) => Accel::new(0, 0, 2048 + 5000),
                    Some(1) => Accel::new(0, 0, 2048 - 2500),
                    Some(2) => Accel::new(0, 0, 2048 + 900),
                    _ => REST,
                };
                let seq = ring.push(a);
                if baseline.track(a, THRESHOLD) > THRESHOLD && crossing.is_none() {
                    crossing = Some(seq);
                }
            }
            ring.stamp(1_000_000 + burst * 20_000);
        }

        assert_eq!(crossing, Some(knock_at));
        let t = ring.time_of(knock_at).unwrap();
        let center = ring.seq_at(t).unwrap();
        assert_eq!(center, knock_at);
        assert!(window_complete(&ring, center, 16));
        let mut w = [Accel::default(); 20];
        extract_window(&ring, center, 4, &mut w).unwrap();
        assert_eq!(w[4], Accel::new(0, 0, 2048 + 5000));
        assert!((peak_dynamic(&w, 4) - 5000.0).abs() < 1e-3);
    }
}
