//! Signal helpers for the four piezo channels: pure functions and small state
//! machines, no HAL, unit-tested on the host.
//!
//! The ADC scans the channels in a fixed order, so the sample stream is
//! *interleaved*: `ch0, ch1, ch2, ch3, ch0, ch1, ...`. One group of
//! [`CHANNELS`] samples is a [`Frame`]. Samples are raw 12-bit counts
//! (0..=[`ADC_MAX`]); the front end idles at mid-scale ([`MID_SCALE`]).
//!
//! - [`frames`], [`channel`], [`deinterleave`]: split the interleaved stream.
//! - [`IdleStats`]: exact running mean / RMS noise / min / max per channel.
//! - [`Trigger`]: "any channel deviates from its own baseline by more than a
//!   threshold", with a slowly tracking baseline.
//! - [`Recorder`]: pre/post-trigger window capture into a ring, optionally
//!   decimated with peak hold; [`Window`] reads the result in time order.
//! - [`onset`], [`onset_time_us`], [`arrival_order`]: arrival time per channel
//!   with scan-skew correction.
//! - [`extremes`]: min / max and flat-top (clamp) detection.
//! - [`envelope`], [`ring_down`]: decay time of a knock.

use crate::stats;

/// Number of piezo channels in one scan.
pub const CHANNELS: usize = 4;

/// Largest 12-bit ADC code (the upper rail).
pub const ADC_MAX: u16 = 4095;

/// Nominal idle level of the mid-rail-biased front end, in counts.
pub const MID_SCALE: u16 = 2048;

/// One scan: one sample per channel, in channel order.
pub type Frame = [u16; CHANNELS];

/// Iterate over the complete frames of an interleaved buffer.
///
/// A trailing partial frame is ignored.
pub fn frames(interleaved: &[u16]) -> impl ExactSizeIterator<Item = Frame> + Clone + '_ {
    interleaved.as_chunks::<CHANNELS>().0.iter().copied()
}

/// Iterate over the samples of one channel of an interleaved buffer.
///
/// Only complete frames are used. An out-of-range `ch` yields nothing.
pub fn channel(interleaved: &[u16], ch: usize) -> impl Iterator<Item = u16> + Clone + '_ {
    frames(interleaved).filter_map(move |frame| frame.get(ch).copied())
}

/// Copy one channel of an interleaved buffer into `out`.
///
/// Returns the number of samples written: the smaller of the number of
/// complete frames and `out.len()`.
pub fn deinterleave(interleaved: &[u16], ch: usize, out: &mut [u16]) -> usize {
    let mut n = 0;
    for (dst, src) in out.iter_mut().zip(channel(interleaved, ch)) {
        *dst = src;
        n += 1;
    }
    n
}

/// Convert raw counts to millivolts for a given full-scale reference.
///
/// `counts * vref_mv / 4095`, rounded to nearest.
pub const fn counts_to_mv(counts: u16, vref_mv: u32) -> u32 {
    (counts as u32 * vref_mv + ADC_MAX as u32 / 2) / ADC_MAX as u32
}

/// Running statistics of one channel, exact in integer arithmetic.
///
/// Unlike [`stats::mean`] this does not lose precision over the hundreds of
/// thousands of samples of a multi-second idle measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelStats {
    n: u32,
    sum: u64,
    sum_sq: u64,
    min: u16,
    max: u16,
}

impl Default for ChannelStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelStats {
    /// An empty accumulator.
    pub const fn new() -> Self {
        Self {
            n: 0,
            sum: 0,
            sum_sq: 0,
            min: u16::MAX,
            max: 0,
        }
    }

    /// Add one sample. Samples beyond `u32::MAX` are ignored.
    pub fn push(&mut self, v: u16) {
        if self.n == u32::MAX {
            return;
        }
        self.n += 1;
        self.sum += u64::from(v);
        self.sum_sq += u64::from(v) * u64::from(v);
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }

    /// Number of samples seen.
    pub const fn count(&self) -> u32 {
        self.n
    }

    /// Mean in counts. `None` if empty.
    pub fn mean(&self) -> Option<f32> {
        (self.n != 0).then(|| (self.sum as f64 / f64::from(self.n)) as f32)
    }

    /// RMS deviation from the mean (population standard deviation) in counts.
    /// `None` if empty.
    pub fn std_dev(&self) -> Option<f32> {
        if self.n == 0 {
            return None;
        }
        // n·Σx² − (Σx)² is exact in u128 and never negative.
        let n = u128::from(self.n);
        let num = n * u128::from(self.sum_sq) - u128::from(self.sum) * u128::from(self.sum);
        let var = num as f64 / (n * n) as f64;
        Some(libm::sqrt(var) as f32)
    }

    /// Smallest sample. `None` if empty.
    pub fn min(&self) -> Option<u16> {
        (self.n != 0).then_some(self.min)
    }

    /// Largest sample. `None` if empty.
    pub fn max(&self) -> Option<u16> {
        (self.n != 0).then_some(self.max)
    }
}

/// Running statistics of all four channels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IdleStats {
    /// Per-channel accumulators, indexed by channel.
    pub ch: [ChannelStats; CHANNELS],
}

impl IdleStats {
    /// Empty accumulators.
    pub const fn new() -> Self {
        Self {
            ch: [ChannelStats::new(); CHANNELS],
        }
    }

    /// Add one frame.
    pub fn push_frame(&mut self, frame: Frame) {
        for (acc, v) in self.ch.iter_mut().zip(frame) {
            acc.push(v);
        }
    }

    /// Add every complete frame of an interleaved buffer.
    pub fn push_interleaved(&mut self, interleaved: &[u16]) {
        for frame in frames(interleaved) {
            self.push_frame(frame);
        }
    }

    /// Number of frames seen.
    pub const fn frames(&self) -> u32 {
        self.ch[0].count()
    }

    /// Per-channel means rounded to counts; [`MID_SCALE`] for an empty channel.
    pub fn means(&self) -> Frame {
        let mut out = [MID_SCALE; CHANNELS];
        for (o, acc) in out.iter_mut().zip(&self.ch) {
            if let Some(m) = acc.mean() {
                *o = libm::roundf(m) as u16;
            }
        }
        out
    }
}

/// Threshold trigger: fires when any channel deviates from its own baseline
/// by more than `threshold` counts.
///
/// The baseline is set from an idle measurement ([`Trigger::set_baseline`])
/// and can follow slow drift ([`Trigger::track`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trigger {
    /// Baselines in 1/256 count (Q8), so slow tracking does not stall.
    baseline_q8: [i32; CHANNELS],
    threshold: u16,
}

impl Trigger {
    /// A trigger with every baseline at [`MID_SCALE`].
    pub const fn new(threshold: u16) -> Self {
        Self {
            baseline_q8: [(MID_SCALE as i32) << 8; CHANNELS],
            threshold,
        }
    }

    /// Threshold in counts (the deviation must be strictly larger).
    pub const fn threshold(&self) -> u16 {
        self.threshold
    }

    /// Set every baseline, in counts.
    pub fn set_baseline(&mut self, baseline: Frame) {
        for (b, v) in self.baseline_q8.iter_mut().zip(baseline) {
            *b = i32::from(v) << 8;
        }
    }

    /// Current baselines rounded to counts.
    pub fn baseline(&self) -> Frame {
        let mut out = [0; CHANNELS];
        for (o, b) in out.iter_mut().zip(self.baseline_q8) {
            *o = ((b + 128) >> 8).clamp(0, i32::from(ADC_MAX)) as u16;
        }
        out
    }

    /// Move each baseline a quarter of the way toward the mean of a *quiet*
    /// interleaved buffer. Does nothing for a buffer without a complete frame.
    pub fn track(&mut self, quiet: &[u16]) {
        let mut stats = IdleStats::new();
        stats.push_interleaved(quiet);
        let n = stats.frames();
        if n == 0 {
            return;
        }
        for (b, acc) in self.baseline_q8.iter_mut().zip(&stats.ch) {
            let mean_q8 = ((acc.sum << 8) / u64::from(n)) as i32;
            *b += (mean_q8 - *b) / 4;
        }
    }

    /// The channel of this frame that deviates most from its baseline, if that
    /// deviation exceeds the threshold.
    pub fn hit(&self, frame: Frame) -> Option<usize> {
        let mut best: Option<(usize, u32)> = None;
        for (ch, (&sample, &baseline)) in frame.iter().zip(&self.baseline_q8).enumerate() {
            // Compare in Q8 so the hot path needs no rounding of the baseline.
            let dev = ((i32::from(sample) << 8) - baseline).unsigned_abs();
            if dev > u32::from(self.threshold) << 8 && best.is_none_or(|(_, d)| dev > d) {
                best = Some((ch, dev));
            }
        }
        best.map(|(ch, _)| ch)
    }

    /// First frame of an interleaved buffer that fires the trigger:
    /// `(frame index, channel)`.
    pub fn find(&self, interleaved: &[u16]) -> Option<(usize, usize)> {
        frames(interleaved)
            .enumerate()
            .find_map(|(i, f)| self.hit(f).map(|ch| (i, ch)))
    }
}

/// Pre/post-trigger window capture.
///
/// Frames are written continuously into a ring. When the caller reports a
/// trigger, the recorder keeps `pre` entries from before the trigger and
/// records `post` more (the first of them holds the trigger frame), then
/// stops; [`Recorder::window`] returns the `pre + post` entries in time order.
///
/// # Decimation
///
/// With `decim > 1` each ring entry stands for `decim` consecutive frames and
/// holds, per channel, the sample of that group that deviates most from the
/// baseline given to [`Recorder::arm`] (*peak hold*). The entry therefore
/// still is a raw count, and the envelope of an oscillation survives the
/// decimation, which plain "every n-th sample" would alias away.
#[derive(Debug)]
pub struct Recorder<'a> {
    ring: &'a mut [u16],
    pre: usize,
    post: usize,
    decim: usize,
    baseline: Frame,
    /// Next entry to write.
    write: usize,
    /// Entries written since `arm`, saturating at the ring capacity.
    stored: usize,
    /// Entries still to write after the trigger; `None` while waiting.
    remaining: Option<usize>,
    bin: Frame,
    bin_len: usize,
}

impl<'a> Recorder<'a> {
    /// Create a recorder for `pre` entries before and `post` entries from the
    /// trigger on, each entry covering `decim` frames.
    ///
    /// Returns `None` if `post` or `decim` is 0 or `storage` holds fewer than
    /// `(pre + post) * CHANNELS` samples.
    pub fn new(storage: &'a mut [u16], pre: usize, post: usize, decim: usize) -> Option<Self> {
        let needed = pre.checked_add(post)?.checked_mul(CHANNELS)?;
        if post == 0 || decim == 0 || storage.len() < needed {
            return None;
        }
        Some(Self {
            ring: &mut storage[..needed],
            pre,
            post,
            decim,
            baseline: [MID_SCALE; CHANNELS],
            write: 0,
            stored: 0,
            remaining: None,
            bin: [0; CHANNELS],
            bin_len: 0,
        })
    }

    /// Ring capacity in entries (`pre + post`).
    pub const fn capacity(&self) -> usize {
        self.pre + self.post
    }

    /// Entries kept from before the trigger.
    pub const fn pre(&self) -> usize {
        self.pre
    }

    /// Frames per entry.
    pub const fn decimation(&self) -> usize {
        self.decim
    }

    /// Forget everything recorded and wait for a new trigger. `baseline` is
    /// the reference for peak-hold decimation.
    pub fn arm(&mut self, baseline: Frame) {
        self.baseline = baseline;
        self.write = 0;
        self.stored = 0;
        self.remaining = None;
        self.bin_len = 0;
    }

    /// True once the trigger has been accepted (recording the post part, or
    /// finished).
    pub const fn triggered(&self) -> bool {
        self.remaining.is_some()
    }

    /// True when the window is complete. Further frames are ignored until
    /// [`Recorder::arm`].
    pub const fn is_done(&self) -> bool {
        matches!(self.remaining, Some(0))
    }

    /// Feed one frame; `hit` tells whether this frame fired the trigger.
    ///
    /// A trigger is only accepted once `pre` entries of history exist, so a
    /// completed window is always full. Returns [`Recorder::is_done`].
    pub fn push(&mut self, frame: Frame, hit: bool) -> bool {
        if self.is_done() {
            return true;
        }
        if self.bin_len == 0 {
            self.bin = frame;
        } else {
            for ((held, &sample), &base) in self.bin.iter_mut().zip(&frame).zip(&self.baseline) {
                if sample.abs_diff(base) > held.abs_diff(base) {
                    *held = sample;
                }
            }
        }
        self.bin_len += 1;
        if hit && self.remaining.is_none() && self.stored >= self.pre {
            self.remaining = Some(self.post);
        }
        if self.bin_len == self.decim {
            self.bin_len = 0;
            let at = self.write * CHANNELS;
            self.ring[at..at + CHANNELS].copy_from_slice(&self.bin);
            self.write = (self.write + 1) % self.capacity();
            self.stored = (self.stored + 1).min(self.capacity());
            if let Some(left) = self.remaining.as_mut() {
                *left -= 1;
            }
        }
        self.is_done()
    }

    /// The captured window, oldest entry first. `None` until
    /// [`Recorder::is_done`].
    pub fn window(&self) -> Option<Window<'_>> {
        self.is_done().then_some(Window {
            ring: self.ring,
            start: self.write,
            len: self.capacity(),
            pre: self.pre,
        })
    }
}

/// A completed capture: `len()` entries in time order; entry [`Window::pre`]
/// contains the trigger.
#[derive(Debug, Clone, Copy)]
pub struct Window<'a> {
    ring: &'a [u16],
    start: usize,
    len: usize,
    pre: usize,
}

impl Window<'_> {
    /// Number of entries.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// True for a window without entries (never produced by [`Recorder`]).
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Index of the entry that contains the trigger frame.
    pub const fn pre(&self) -> usize {
        self.pre
    }

    /// Entry `i` (0 = oldest). `None` past the end.
    pub fn frame(&self, i: usize) -> Option<Frame> {
        if i >= self.len {
            return None;
        }
        let at = ((self.start + i) % self.len) * CHANNELS;
        let s = &self.ring[at..at + CHANNELS];
        Some([s[0], s[1], s[2], s[3]])
    }

    /// All entries in time order.
    pub fn frames(&self) -> impl Iterator<Item = Frame> + '_ {
        (0..self.len).filter_map(|i| self.frame(i))
    }

    /// Copy one channel in time order into `out` as signed samples (12-bit
    /// counts always fit), ready for the [`stats`] functions.
    ///
    /// Returns the number of samples written (`min(len, out.len())`; 0 for an
    /// out-of-range channel).
    pub fn copy_channel(&self, ch: usize, out: &mut [i16]) -> usize {
        if ch >= CHANNELS {
            return 0;
        }
        let mut n = 0;
        for (dst, frame) in out.iter_mut().zip(self.frames()) {
            *dst = frame[ch] as i16;
            n += 1;
        }
        n
    }
}

/// Result of [`onset`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Onset {
    /// Index of the first sample beyond the threshold.
    pub index: usize,
    /// Sub-sample position of the threshold crossing, linearly interpolated
    /// between `index - 1` and `index`; in `(index - 1, index]`.
    pub position: f32,
    /// The threshold that was used, in counts.
    pub threshold: f32,
}

/// Onset of a knock on one channel: the first sample after the baseline whose
/// deviation from the baseline mean exceeds `max(k_sigma · σ, min_threshold)`.
///
/// The baseline is `samples[..baseline_len]` (the pre-trigger part of a
/// window). This is [`stats::onset_index`] with a floor on the threshold (a
/// quiet 12-bit channel can have σ well below one count, and k·σ alone would
/// then fire on a single LSB of noise) and with sub-sample interpolation.
///
/// Returns `None` if `baseline_len` is 0 or not smaller than the slice
/// length, or if nothing crosses the threshold.
pub fn onset(
    samples: &[i16],
    baseline_len: usize,
    k_sigma: f32,
    min_threshold: f32,
) -> Option<Onset> {
    if baseline_len == 0 || baseline_len >= samples.len() {
        return None;
    }
    let baseline = &samples[..baseline_len];
    let mean = stats::mean(baseline)?;
    let threshold = (k_sigma * stats::std_dev(baseline)?).max(min_threshold);
    let dev = |i: usize| libm::fabsf(f32::from(samples[i]) - mean);
    let index = (baseline_len..samples.len()).find(|&i| dev(i) > threshold)?;
    let (before, after) = (dev(index - 1), dev(index));
    // `after > threshold`; if the previous sample is below the threshold the
    // crossing lies between the two.
    let fraction = if before < threshold {
        (threshold - before) / (after - before)
    } else {
        1.0
    };
    Some(Onset {
        index,
        position: (index - 1) as f32 + fraction,
        threshold,
    })
}

/// Time of an onset relative to the trigger, in microseconds, corrected for
/// the scan skew.
///
/// `position` is a (fractional) sample index in a window whose entry
/// `trigger_index` holds the trigger, `period_us` the per-channel sample
/// period, and `skew_us` the delay between consecutive channels of one scan:
/// channel `ch` is sampled `ch · skew_us` after channel 0 of the same frame.
pub fn onset_time_us(
    position: f32,
    trigger_index: usize,
    period_us: f32,
    ch: usize,
    skew_us: f32,
) -> f32 {
    (position - trigger_index as f32) * period_us + ch as f32 * skew_us
}

/// Channels sorted by arrival time, earliest first.
///
/// Returns the order and the number of valid entries in it; channels without
/// an onset (`None` or NaN) are left out. Ties keep channel order.
pub fn arrival_order(times_us: &[Option<f32>; CHANNELS]) -> ([usize; CHANNELS], usize) {
    let mut order = [0; CHANNELS];
    let mut n = 0;
    for (ch, t) in times_us.iter().enumerate() {
        let Some(t) = t.filter(|t| !t.is_nan()) else {
            continue;
        };
        // Insertion sort: find the first later arrival and shift the rest.
        let at = order[..n]
            .iter()
            .position(|&o| times_us[o].is_some_and(|other| other > t))
            .unwrap_or(n);
        order.copy_within(at..n, at + 1);
        order[at] = ch;
        n += 1;
    }
    (order, n)
}

/// Extremes of one channel of a window, with the longest runs of consecutive
/// samples that stay close to each extreme. See [`extremes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extremes {
    /// Smallest sample.
    pub min: i16,
    /// Largest sample.
    pub max: i16,
    /// Longest run of consecutive samples within the tolerance of `min`.
    pub min_run: usize,
    /// Longest run of consecutive samples within the tolerance of `max`.
    pub max_run: usize,
}

impl Extremes {
    /// Largest absolute deviation from `baseline`, in counts.
    pub fn peak_dev(&self, baseline: f32) -> f32 {
        libm::fabsf(f32::from(self.max) - baseline).max(libm::fabsf(f32::from(self.min) - baseline))
    }

    /// True if the top of the waveform is flat: at least `min_run` consecutive
    /// samples near the maximum, and the maximum at least `min_dev` counts
    /// above `baseline` (so an idle channel is not reported).
    pub fn flat_top_high(&self, baseline: f32, min_dev: f32, min_run: usize) -> bool {
        self.max_run >= min_run && f32::from(self.max) - baseline >= min_dev
    }

    /// Same as [`Extremes::flat_top_high`] for the bottom of the waveform.
    pub fn flat_top_low(&self, baseline: f32, min_dev: f32, min_run: usize) -> bool {
        self.min_run >= min_run && baseline - f32::from(self.min) >= min_dev
    }

    /// True if any sample sits on an ADC rail (0 or [`ADC_MAX`]).
    pub fn at_rail(&self) -> bool {
        self.min <= 0 || self.max >= ADC_MAX as i16
    }
}

/// Min, max and flat-top run lengths of a sample window.
///
/// A clamped (or railed) knock shows up as several consecutive samples within
/// `tolerance` counts of the extreme instead of a single rounded peak.
/// `None` for an empty slice.
pub fn extremes(samples: &[i16], tolerance: i16) -> Option<Extremes> {
    let min = *samples.iter().min()?;
    let max = *samples.iter().max()?;
    let tolerance = i32::from(tolerance);
    let (mut min_run, mut max_run) = (0usize, 0usize);
    let (mut lo, mut hi) = (0usize, 0usize);
    for &s in samples {
        lo = if i32::from(s) - i32::from(min) <= tolerance {
            lo + 1
        } else {
            0
        };
        hi = if i32::from(max) - i32::from(s) <= tolerance {
            hi + 1
        } else {
            0
        };
        min_run = min_run.max(lo);
        max_run = max_run.max(hi);
    }
    Some(Extremes {
        min,
        max,
        min_run,
        max_run,
    })
}

/// Envelope of a signal: the largest absolute deviation from `baseline` in
/// each group of `bin` consecutive samples, written to `out` in counts.
///
/// Returns the number of envelope points written. A trailing partial group is
/// included. Returns 0 if `bin` is 0.
pub fn envelope(samples: &[i16], baseline: f32, bin: usize, out: &mut [f32]) -> usize {
    if bin == 0 {
        return 0;
    }
    let mut n = 0;
    for (dst, group) in out.iter_mut().zip(samples.chunks(bin)) {
        *dst = stats::peak_abs_dev(group, baseline).unwrap_or(0.0);
        n += 1;
    }
    n
}

/// Outcome of [`ring_down`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingDown {
    /// Nothing exceeded the threshold from `start` on.
    Quiet,
    /// The signal fell below the threshold for good; `last_above` is the index
    /// of the last sample beyond it.
    Settled {
        /// Index of the last sample beyond the threshold.
        last_above: usize,
    },
    /// The signal was still (or again) beyond the threshold within the last
    /// `quiet_len` samples: the window is too short to tell.
    StillRinging {
        /// Index of the last sample beyond the threshold.
        last_above: usize,
    },
}

/// Find where a knock has rung down: the last sample of `samples[start..]`
/// that deviates from `baseline` by more than `threshold`.
///
/// The result is [`RingDown::Settled`] only if at least `quiet_len` samples
/// follow that one. With peak-hold decimated input (see [`Recorder`]) each
/// sample already is the envelope of its group, so no rectification or
/// smoothing is needed.
pub fn ring_down(
    samples: &[i16],
    baseline: f32,
    threshold: f32,
    start: usize,
    quiet_len: usize,
) -> RingDown {
    let above = |s: &i16| libm::fabsf(f32::from(*s) - baseline) > threshold;
    let Some(tail) = samples.get(start..) else {
        return RingDown::Quiet;
    };
    match tail.iter().rposition(above) {
        None => RingDown::Quiet,
        Some(i) => {
            let last_above = start + i;
            if samples.len() - 1 - last_above >= quiet_len {
                RingDown::Settled { last_above }
            } else {
                RingDown::StillRinging { last_above }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3 * b.abs().max(1.0)
    }

    /// Interleave four per-channel sequences of equal length.
    fn interleave(chs: [&[u16]; CHANNELS]) -> Vec<u16> {
        (0..chs[0].len()).flat_map(|i| chs.map(|c| c[i])).collect()
    }

    #[test]
    fn frames_and_channels_deinterleave_in_scan_order() {
        let buf = [10, 20, 30, 40, 11, 21, 31, 41, 12, 22, 32, 42, 99, 98];
        let f: Vec<Frame> = frames(&buf).collect();
        assert_eq!(f, [[10, 20, 30, 40], [11, 21, 31, 41], [12, 22, 32, 42]]);
        assert_eq!(channel(&buf, 0).collect::<Vec<_>>(), [10, 11, 12]);
        assert_eq!(channel(&buf, 3).collect::<Vec<_>>(), [40, 41, 42]);
        // The trailing partial frame (99, 98) is never used.
        assert_eq!(channel(&buf, 1).collect::<Vec<_>>(), [20, 21, 22]);
        assert_eq!(channel(&buf, 4).count(), 0);
    }

    #[test]
    fn deinterleave_is_bounded_by_both_lengths() {
        let buf = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let mut out = [0u16; 8];
        assert_eq!(deinterleave(&buf, 2, &mut out), 3);
        assert_eq!(&out[..3], &[3, 7, 11]);
        let mut short = [0u16; 2];
        assert_eq!(deinterleave(&buf, 1, &mut short), 2);
        assert_eq!(short, [2, 6]);
        assert_eq!(deinterleave(&buf, 7, &mut out), 0);
    }

    #[test]
    fn counts_to_millivolts() {
        assert_eq!(counts_to_mv(0, 3300), 0);
        assert_eq!(counts_to_mv(4095, 3300), 3300);
        assert_eq!(counts_to_mv(2048, 3300), 1650);
    }

    #[test]
    fn channel_stats_are_exact() {
        let mut s = ChannelStats::new();
        assert_eq!(s.mean(), None);
        assert_eq!(s.std_dev(), None);
        assert_eq!(s.min(), None);
        for v in [2047u16, 2049, 2047, 2049] {
            s.push(v);
        }
        assert_eq!(s.count(), 4);
        assert!(close(s.mean().unwrap(), 2048.0));
        assert!(close(s.std_dev().unwrap(), 1.0));
        assert_eq!((s.min(), s.max()), (Some(2047), Some(2049)));
    }

    #[test]
    fn channel_stats_keep_precision_over_a_long_run() {
        // 500 000 samples alternating 2047 / 2050: f32 accumulation would
        // drift, the integer sums do not.
        let mut s = ChannelStats::new();
        for i in 0..500_000u32 {
            s.push(if i % 2 == 0 { 2047 } else { 2050 });
        }
        assert!((s.mean().unwrap() - 2048.5).abs() < 1e-3);
        assert!((s.std_dev().unwrap() - 1.5).abs() < 1e-4);
    }

    #[test]
    fn idle_stats_split_channels() {
        let buf = interleave([&[100, 102], &[200, 200], &[300, 310], &[4095, 0]]);
        let mut s = IdleStats::new();
        s.push_interleaved(&buf);
        assert_eq!(s.frames(), 2);
        assert_eq!(s.means(), [101, 200, 305, 2048]);
        assert!(close(s.ch[2].std_dev().unwrap(), 5.0));
        assert_eq!(s.ch[3].max(), Some(4095));
        assert_eq!(IdleStats::new().means(), [MID_SCALE; CHANNELS]);
    }

    #[test]
    fn trigger_uses_each_channels_own_baseline() {
        let mut t = Trigger::new(50);
        t.set_baseline([2000, 2100, 2048, 1900]);
        assert_eq!(t.baseline(), [2000, 2100, 2048, 1900]);
        // Exactly at the threshold does not fire; one more does.
        assert_eq!(t.hit([2050, 2100, 2048, 1900]), None);
        assert_eq!(t.hit([2051, 2100, 2048, 1900]), Some(0));
        // Negative excursions count, and 2100 is idle for ch1 only.
        assert_eq!(t.hit([2000, 2100, 2048, 1849]), Some(3));
        assert_eq!(t.hit([2100, 2100, 2048, 1900]), Some(0));
        // The largest deviation wins.
        assert_eq!(t.hit([2060, 2300, 2048, 1900]), Some(1));
    }

    #[test]
    fn trigger_finds_the_first_frame() {
        let mut t = Trigger::new(20);
        t.set_baseline([2048; 4]);
        let buf = interleave([
            &[2048, 2050, 2048, 2048],
            &[2048, 2048, 2048, 2048],
            &[2048, 2048, 2000, 1000],
            &[2048, 2048, 2048, 2048],
        ]);
        assert_eq!(t.find(&buf), Some((2, 2)));
        assert_eq!(t.find(&buf[..8]), None);
    }

    #[test]
    fn trigger_baseline_tracks_slow_drift() {
        let mut t = Trigger::new(50);
        t.set_baseline([2048; 4]);
        let drifted = [2060u16, 2048, 2040, 2048].repeat(64);
        for _ in 0..40 {
            t.track(&drifted);
        }
        assert_eq!(t.baseline(), [2060, 2048, 2040, 2048]);
        // One step moves a quarter of the way.
        let mut t = Trigger::new(50);
        t.set_baseline([2000; 4]);
        t.track(&[2040u16; 16]);
        assert_eq!(t.baseline(), [2010; 4]);
        t.track(&[]);
        assert_eq!(t.baseline(), [2010; 4]);
    }

    /// Frame whose channel `ch` holds `1000 * (ch + 1) + i`.
    fn numbered(i: u16) -> Frame {
        [1000 + i, 2000 + i, 3000 + i, 4000 + i]
    }

    #[test]
    fn recorder_rejects_bad_geometry() {
        let mut small = [0u16; 39];
        assert!(Recorder::new(&mut small, 4, 6, 1).is_none());
        let mut ok = [0u16; 40];
        assert!(Recorder::new(&mut ok, 4, 0, 1).is_none());
        assert!(Recorder::new(&mut ok, 4, 6, 0).is_none());
        assert!(Recorder::new(&mut ok, 4, 6, 1).is_some());
    }

    #[test]
    fn recorder_keeps_pre_and_post_around_the_trigger() {
        let mut storage = [0u16; 64];
        let mut rec = Recorder::new(&mut storage, 3, 5, 1).unwrap();
        rec.arm([2048; 4]);
        assert!(rec.window().is_none());
        // Trigger at frame 10: the ring has wrapped by then.
        let mut done_at = None;
        for i in 0..40u16 {
            if rec.push(numbered(i), i == 10) && done_at.is_none() {
                done_at = Some(i);
            }
        }
        // 5 post entries: frames 10..=14.
        assert_eq!(done_at, Some(14));
        let w = rec.window().unwrap();
        assert_eq!((w.len(), w.pre()), (8, 3));
        let got: Vec<Frame> = w.frames().collect();
        let want: Vec<Frame> = (7..15).map(numbered).collect();
        assert_eq!(got, want);
        assert_eq!(w.frame(w.pre()), Some(numbered(10)));
        assert_eq!(w.frame(8), None);

        let mut ch2 = [0i16; 16];
        assert_eq!(w.copy_channel(2, &mut ch2), 8);
        assert_eq!(&ch2[..8], &[3007, 3008, 3009, 3010, 3011, 3012, 3013, 3014]);
        assert_eq!(w.copy_channel(4, &mut ch2), 0);
    }

    #[test]
    fn recorder_ignores_triggers_before_the_history_is_full() {
        let mut storage = [0u16; 64];
        let mut rec = Recorder::new(&mut storage, 4, 2, 1).unwrap();
        rec.arm([2048; 4]);
        // Hits at frames 1 and 2 come too early (fewer than 4 stored).
        for i in 0..4u16 {
            assert!(!rec.push(numbered(i), i == 1 || i == 2));
            assert!(!rec.triggered());
        }
        assert!(!rec.push(numbered(4), true));
        assert!(rec.triggered());
        assert!(rec.push(numbered(5), false));
        let got: Vec<Frame> = rec.window().unwrap().frames().collect();
        let want: Vec<Frame> = (0..6).map(numbered).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn recorder_can_be_rearmed() {
        let mut storage = [0u16; 16];
        let mut rec = Recorder::new(&mut storage, 1, 2, 1).unwrap();
        rec.arm([2048; 4]);
        for i in 0..3u16 {
            rec.push(numbered(i), i == 1);
        }
        assert!(rec.is_done());
        // Frames after completion do not disturb the window.
        assert!(rec.push(numbered(99), true));
        assert_eq!(rec.window().unwrap().frame(1), Some(numbered(1)));

        rec.arm([2048; 4]);
        assert!(!rec.is_done() && !rec.triggered());
        for i in 20..26u16 {
            rec.push(numbered(i), i == 24);
        }
        let got: Vec<Frame> = rec.window().unwrap().frames().collect();
        assert_eq!(got, [numbered(23), numbered(24), numbered(25)]);
    }

    #[test]
    fn recorder_decimates_with_peak_hold() {
        let mut storage = [0u16; 32];
        let mut rec = Recorder::new(&mut storage, 1, 2, 4).unwrap();
        rec.arm([2000; 4]);
        // Three groups of four frames; the trigger falls in the second.
        let quiet = [2000u16; 4];
        let groups: [[Frame; 4]; 3] = [
            [
                [2000, 2001, 1999, 2000],
                [2003, 2000, 2000, 2000],
                quiet,
                [1998, 2000, 2000, 2000],
            ],
            [
                quiet,
                [2500, 1500, 2000, 2010],
                [2400, 1400, 2000, 1980],
                quiet,
            ],
            [
                [2100, 1900, 2000, 2000],
                [1850, 2090, 2001, 2000],
                quiet,
                quiet,
            ],
        ];
        let mut done = false;
        for (g, group) in groups.iter().enumerate() {
            for (i, frame) in group.iter().enumerate() {
                done = rec.push(*frame, g == 1 && i == 1);
            }
        }
        assert!(done);
        let got: Vec<Frame> = rec.window().unwrap().frames().collect();
        assert_eq!(
            got,
            [
                // Largest deviation from 2000 per channel; first one wins ties.
                [2003, 2001, 1999, 2000],
                [2500, 1400, 2000, 1980],
                [1850, 1900, 2001, 2000],
            ]
        );
    }

    #[test]
    fn onset_interpolates_the_crossing() {
        // Baseline 2048 ± 1 (σ = 1); threshold = 8σ = 8.
        let mut s = vec![2047i16, 2049, 2047, 2049, 2047, 2049, 2047, 2049];
        s.extend([2048, 2052, 2064, 2200]);
        let o = onset(&s, 8, 8.0, 2.0).unwrap();
        assert_eq!(o.index, 10);
        assert!(close(o.threshold, 8.0));
        // Deviations 4 → 16 cross 8 a third of the way from sample 9 to 10.
        assert!(close(o.position, 9.0 + 4.0 / 12.0));
        // Same index as the plain helper when the floor is not binding.
        assert_eq!(stats::onset_index(&s, 8, 8.0), Some(10));
    }

    #[test]
    fn onset_floor_stops_single_lsb_noise() {
        // Flat baseline (σ = 0): without the floor the 1-count blip would fire.
        let s = [2048i16, 2048, 2048, 2048, 2049, 2048, 2047, 2060, 2100];
        assert_eq!(stats::onset_index(&s, 4, 8.0), Some(4));
        let o = onset(&s, 4, 8.0, 5.0).unwrap();
        assert_eq!(o.index, 7);
        assert!(close(o.threshold, 5.0));
        // Deviations 1 → 12 cross 5 at 4/11 of the step.
        assert!(close(o.position, 6.0 + 4.0 / 11.0));
    }

    #[test]
    fn onset_handles_negative_steps_and_edges() {
        let s = [2048i16, 2048, 2048, 1900, 1800];
        let o = onset(&s, 3, 8.0, 4.0).unwrap();
        assert_eq!(o.index, 3);
        assert!(o.position > 2.0 && o.position <= 3.0);
        assert_eq!(onset(&s, 0, 8.0, 4.0), None);
        assert_eq!(onset(&s, 5, 8.0, 4.0), None);
        assert_eq!(onset(&[2048i16; 16], 4, 8.0, 4.0), None);
    }

    #[test]
    fn onset_time_applies_the_scan_skew() {
        // 100 kS/s per channel (10 µs), 2.5 µs between channels.
        assert!(close(onset_time_us(100.0, 100, 10.0, 0, 2.5), 0.0));
        assert!(close(onset_time_us(100.0, 100, 10.0, 3, 2.5), 7.5));
        assert!(close(onset_time_us(112.5, 100, 10.0, 1, 2.5), 127.5));
        assert!(close(onset_time_us(95.0, 100, 10.0, 2, 2.5), -45.0));
        // The same physical instant seen on ch0 and ch2: ch2 crosses half a
        // sample "earlier" in index terms because it is sampled 5 µs later.
        let t0 = onset_time_us(110.5, 100, 10.0, 0, 2.5);
        let t2 = onset_time_us(110.0, 100, 10.0, 2, 2.5);
        assert!(close(t0, t2));
    }

    #[test]
    fn arrival_order_sorts_and_skips_missing() {
        let (order, n) = arrival_order(&[Some(30.0), Some(-5.0), None, Some(12.5)]);
        assert_eq!((&order[..n], n), (&[1usize, 3, 0][..], 3));
        let (order, n) = arrival_order(&[Some(1.0), Some(1.0), Some(0.5), Some(f32::NAN)]);
        assert_eq!(&order[..n], &[2, 0, 1]);
        let (_, n) = arrival_order(&[None; 4]);
        assert_eq!(n, 0);
    }

    #[test]
    fn extremes_detect_a_flat_top() {
        // A clipped positive half wave: five samples within 3 counts of 3990.
        let s = [
            2048i16, 2600, 3400, 3988, 3990, 3989, 3990, 3988, 3300, 2500, 1700, 1500, 1900,
        ];
        let e = extremes(&s, 3).unwrap();
        assert_eq!((e.min, e.max), (1500, 3990));
        assert_eq!((e.min_run, e.max_run), (1, 5));
        assert!(close(e.peak_dev(2048.0), 1942.0));
        assert!(e.flat_top_high(2048.0, 200.0, 4));
        assert!(!e.flat_top_high(2048.0, 200.0, 6));
        assert!(!e.flat_top_low(2048.0, 200.0, 4));
        assert!(!e.at_rail());
    }

    #[test]
    fn extremes_do_not_flag_an_idle_channel() {
        // Every sample is "near the extreme" on an idle channel; the minimum
        // deviation keeps that from counting as a clamp.
        let s = [2048i16, 2049, 2047, 2048, 2049, 2048];
        let e = extremes(&s, 3).unwrap();
        assert_eq!((e.min_run, e.max_run), (6, 6));
        assert!(!e.flat_top_high(2048.0, 200.0, 4));
        assert!(!e.flat_top_low(2048.0, 200.0, 4));
        assert_eq!(extremes(&[], 3), None);
    }

    #[test]
    fn extremes_report_the_rails() {
        let low = extremes(&[2048i16, 900, 0, 0, 0, 0, 700], 3).unwrap();
        assert!(low.at_rail());
        assert!(low.flat_top_low(2048.0, 200.0, 4));
        let high = extremes(&[2048i16, 4095, 2048], 3).unwrap();
        assert!(high.at_rail());
        assert!(!high.flat_top_high(2048.0, 200.0, 4));
    }

    #[test]
    fn envelope_takes_the_peak_of_each_group() {
        let s = [2048i16, 2058, 2040, 2048, 2148, 1900, 2048, 2049, 2050];
        let mut out = [0f32; 8];
        let n = envelope(&s, 2048.0, 4, &mut out);
        assert_eq!(n, 3);
        assert!(close(out[0], 10.0) && close(out[1], 148.0) && close(out[2], 2.0));
        assert_eq!(envelope(&s, 2048.0, 0, &mut out), 0);
        let mut one = [0f32; 1];
        assert_eq!(envelope(&s, 2048.0, 4, &mut one), 1);
    }

    #[test]
    fn ring_down_finds_the_last_excursion() {
        // Decaying alternation around 2048, threshold 20 counts.
        let s = [
            2048i16, 2048, 2500, 1700, 2300, 1900, 2100, 2020, 2060, 2040, 2050, 2047, 2049, 2048,
        ];
        assert_eq!(
            ring_down(&s, 2048.0, 20.0, 2, 4),
            RingDown::Settled { last_above: 7 }
        );
        // Not enough quiet samples after it to be sure.
        assert_eq!(
            ring_down(&s, 2048.0, 20.0, 2, 7),
            RingDown::StillRinging { last_above: 7 }
        );
        assert_eq!(
            ring_down(&s[..6], 2048.0, 20.0, 2, 1),
            RingDown::StillRinging { last_above: 5 }
        );
    }

    #[test]
    fn ring_down_quiet_cases() {
        let s = [2048i16, 2600, 2048, 2049, 2047];
        // The excursion lies before `start`.
        assert_eq!(ring_down(&s, 2048.0, 20.0, 2, 1), RingDown::Quiet);
        assert_eq!(ring_down(&s, 2048.0, 20.0, 9, 1), RingDown::Quiet);
        assert_eq!(ring_down(&[], 2048.0, 20.0, 0, 1), RingDown::Quiet);
    }
}
