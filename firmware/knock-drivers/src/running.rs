//! Streaming statistics for long recordings (T6: 60 s × 400 Hz per sensor),
//! where the samples cannot be kept in RAM.
//!
//! Sums are accumulated exactly in integers (`i64` sum, `u64` sum of squares:
//! at most 2³⁰ per sample, so more than 10¹⁰ samples fit) and only the final
//! mean and spread are computed in floating point, in `f64` to survive the
//! cancellation in `E[x²] − E[x]²`.

use crate::Accel;

/// Running count, sum and sum of squares of `i16` samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accum {
    n: u32,
    sum: i64,
    sum_sq: u64,
}

impl Accum {
    /// An empty accumulator.
    pub const fn new() -> Self {
        Self {
            n: 0,
            sum: 0,
            sum_sq: 0,
        }
    }

    /// Add one sample.
    pub const fn push(&mut self, v: i16) {
        let v = v as i64;
        self.n = self.n.saturating_add(1);
        self.sum += v;
        self.sum_sq += (v * v) as u64;
    }

    /// Number of samples.
    pub const fn count(&self) -> u32 {
        self.n
    }

    fn mean_f64(&self) -> Option<f64> {
        (self.n != 0).then(|| self.sum as f64 / f64::from(self.n))
    }

    /// Population variance in `f64`; `None` when empty.
    pub fn variance(&self) -> Option<f64> {
        let mean = self.mean_f64()?;
        let var = self.sum_sq as f64 / f64::from(self.n) - mean * mean;
        // Rounding can leave a tiny negative number for a constant signal.
        Some(if var < 0.0 { 0.0 } else { var })
    }

    /// Arithmetic mean; `None` when empty.
    pub fn mean(&self) -> Option<f32> {
        self.mean_f64().map(|m| m as f32)
    }

    /// Population standard deviation (RMS about the mean); `None` when empty.
    pub fn std_dev(&self) -> Option<f32> {
        self.variance().map(|v| libm::sqrt(v) as f32)
    }
}

/// Running statistics of a 3-axis signal: one [`Accum`] per axis plus the mean
/// vector magnitude.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AccelAccum {
    /// X axis.
    pub x: Accum,
    /// Y axis.
    pub y: Accum,
    /// Z axis.
    pub z: Accum,
    /// Sum of |a| in 1/256 LSB.
    mag_sum_q8: u64,
}

impl AccelAccum {
    /// An empty accumulator.
    pub const fn new() -> Self {
        Self {
            x: Accum::new(),
            y: Accum::new(),
            z: Accum::new(),
            mag_sum_q8: 0,
        }
    }

    /// Add one sample.
    pub fn push(&mut self, a: Accel) {
        self.x.push(a.x);
        self.y.push(a.y);
        self.z.push(a.z);
        // |a| <= 56756 LSB, so the Q8 value fits in 24 bits.
        self.mag_sum_q8 += (a.magnitude() * 256.0) as u64;
    }

    /// Number of samples.
    pub const fn count(&self) -> u32 {
        self.x.count()
    }

    /// The axis accumulators in the order x, y, z.
    pub const fn axes(&self) -> [Accum; 3] {
        [self.x, self.y, self.z]
    }

    /// Mean of the vector magnitude |a| (≈ 1 g at rest), in LSB.
    pub fn mean_magnitude(&self) -> Option<f32> {
        let n = self.count();
        (n != 0).then(|| (self.mag_sum_q8 as f64 / 256.0 / f64::from(n)) as f32)
    }

    /// RMS of the dynamic magnitude |a − ā|, in LSB.
    ///
    /// mean(|a − ā|²) = σx² + σy² + σz², so this needs no second pass.
    pub fn dynamic_rms(&self) -> Option<f32> {
        let var = self.x.variance()? + self.y.variance()? + self.z.variance()?;
        Some(libm::sqrt(var) as f32)
    }
}

/// Running minimum, maximum and mean of a non-negative quantity (loop times
/// in microseconds, for example).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MinMaxMean {
    n: u64,
    sum: u64,
    min: u64,
    max: u64,
}

impl MinMaxMean {
    /// No values yet.
    pub const fn new() -> Self {
        Self {
            n: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
        }
    }

    /// Add one value.
    pub const fn push(&mut self, v: u64) {
        self.n += 1;
        self.sum = self.sum.saturating_add(v);
        if v < self.min {
            self.min = v;
        }
        if v > self.max {
            self.max = v;
        }
    }

    /// Number of values.
    pub const fn count(&self) -> u64 {
        self.n
    }

    /// Smallest value (0 when empty).
    pub const fn min(&self) -> u64 {
        if self.n == 0 { 0 } else { self.min }
    }

    /// Largest value (0 when empty).
    pub const fn max(&self) -> u64 {
        self.max
    }

    /// Mean, rounded down (0 when empty).
    pub const fn mean(&self) -> u64 {
        match self.sum.checked_div(self.n) {
            Some(mean) => mean,
            None => 0,
        }
    }
}

/// Median of the values in `values` (the slice is sorted in place).
/// `None` when empty; the mean of the two middle values for an even count.
pub fn median(values: &mut [f32]) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable_by(f32::total_cmp);
    let mid = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[mid]
    } else {
        (values[mid - 1] + values[mid]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats;

    #[test]
    fn empty_accumulators_have_no_answer() {
        let a = Accum::new();
        assert_eq!(a.mean(), None);
        assert_eq!(a.std_dev(), None);
        let v = AccelAccum::new();
        assert_eq!(v.mean_magnitude(), None);
        assert_eq!(v.dynamic_rms(), None);
    }

    #[test]
    fn matches_the_slice_statistics() {
        let data: [i16; 8] = [2040, 2051, 2047, 2049, 2046, 2055, 2043, 2048];
        let mut a = Accum::new();
        for v in data {
            a.push(v);
        }
        assert_eq!(a.count(), 8);
        assert!((a.mean().unwrap() - stats::mean(&data).unwrap()).abs() < 1e-3);
        assert!((a.std_dev().unwrap() - stats::std_dev(&data).unwrap()).abs() < 1e-3);
    }

    #[test]
    fn constant_signal_has_zero_spread() {
        let mut a = Accum::new();
        for _ in 0..24_000 {
            a.push(2047);
        }
        assert_eq!(a.mean(), Some(2047.0));
        assert_eq!(a.std_dev(), Some(0.0));
    }

    #[test]
    fn long_noisy_run_keeps_precision() {
        // 60 s at 400 Hz of 2048 ± 3 alternating: mean 2048, sigma exactly 3.
        let mut a = Accum::new();
        for i in 0..24_000 {
            a.push(if i % 2 == 0 { 2051 } else { 2045 });
        }
        assert!((a.mean().unwrap() - 2048.0).abs() < 1e-4);
        assert!((a.std_dev().unwrap() - 3.0).abs() < 1e-4);
    }

    #[test]
    fn extremes_do_not_overflow() {
        let mut a = Accum::new();
        for _ in 0..100_000 {
            a.push(i16::MIN);
        }
        assert_eq!(a.mean(), Some(-32768.0));
        assert_eq!(a.std_dev(), Some(0.0));
    }

    #[test]
    fn vector_statistics() {
        let mut v = AccelAccum::new();
        // Gravity on z with ±2 on x, ±1 on y, ±2 on z around the mean.
        for i in 0..1000 {
            let s = if i % 2 == 0 { 1 } else { -1 };
            v.push(Accel::new(2 * s, s, 2048 + 2 * s));
        }
        assert_eq!(v.count(), 1000);
        assert!((v.mean_magnitude().unwrap() - 2048.0).abs() < 0.05);
        // sqrt(4 + 1 + 4) = 3
        assert!((v.dynamic_rms().unwrap() - 3.0).abs() < 1e-4);
        assert_eq!(v.axes()[2].mean(), Some(2048.0));
    }

    #[test]
    fn min_max_mean() {
        let mut m = MinMaxMean::new();
        assert_eq!((m.count(), m.min(), m.mean(), m.max()), (0, 0, 0, 0));
        for v in [12_000, 11_500, 19_000, 12_500] {
            m.push(v);
        }
        assert_eq!(
            (m.count(), m.min(), m.mean(), m.max()),
            (4, 11_500, 13_750, 19_000)
        );
    }

    #[test]
    fn median_of_odd_even_and_empty() {
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [3.0]), Some(3.0));
        assert_eq!(median(&mut [5.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(&mut [4.0, 1.0, 3.0, 2.0]), Some(2.5));
        let mut nine = [1.0, 1.1, 0.9, 1.0, 9.0, 1.2, 0.8, 1.0, 1.05];
        assert_eq!(median(&mut nine), Some(1.0));
    }
}
