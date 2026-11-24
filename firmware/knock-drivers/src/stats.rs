//! Small statistics helpers for sample windows. Pure functions, no allocation.
//!
//! Every slice function is generic over [`Sample`], implemented for `i16` (raw
//! sensor data) and `f32`. Results are `f32`; functions that have no meaningful
//! answer for an empty slice return `None`.
//!
//! Accumulation is done in `f32`, which is exact enough for the window sizes
//! used here (a few thousand samples); the spread is computed around the mean
//! in a second pass to avoid cancellation.

/// A sample type the statistics functions accept.
pub trait Sample: Copy {
    /// The sample as `f32`.
    fn to_f32(self) -> f32;
}

impl Sample for i16 {
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
}

impl Sample for f32 {
    fn to_f32(self) -> f32 {
        self
    }
}

/// Arithmetic mean. `None` for an empty slice.
pub fn mean<T: Sample>(samples: &[T]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let sum: f32 = samples.iter().map(|s| s.to_f32()).sum();
    Some(sum / samples.len() as f32)
}

/// Root mean square about zero: `sqrt(mean(x²))`. `None` for an empty slice.
///
/// For a signal with a DC offset (gravity, the piezo mid-rail) this includes
/// the offset; use [`std_dev`] for the noise around the mean.
pub fn rms<T: Sample>(samples: &[T]) -> Option<f32> {
    if samples.is_empty() {
        return None;
    }
    let sum_sq: f32 = samples
        .iter()
        .map(|s| {
            let v = s.to_f32();
            v * v
        })
        .sum();
    Some(libm::sqrtf(sum_sq / samples.len() as f32))
}

/// Population standard deviation: the RMS of the deviation from the mean
/// ("RMS noise"). `None` for an empty slice; 0 for a single sample.
pub fn std_dev<T: Sample>(samples: &[T]) -> Option<f32> {
    let m = mean(samples)?;
    let sum_sq: f32 = samples
        .iter()
        .map(|s| {
            let d = s.to_f32() - m;
            d * d
        })
        .sum();
    Some(libm::sqrtf(sum_sq / samples.len() as f32))
}

/// Largest absolute value. `None` for an empty slice.
///
/// `i16::MIN` gives 32768.0, which does not fit in an `i16`.
pub fn peak_abs<T: Sample>(samples: &[T]) -> Option<f32> {
    samples
        .iter()
        .map(|s| libm::fabsf(s.to_f32()))
        .reduce(f32::max)
}

/// Largest absolute deviation from `reference`. `None` for an empty slice.
///
/// With `reference` set to the pre-trigger baseline this is the peak dynamic
/// amplitude of a knock.
pub fn peak_abs_dev<T: Sample>(samples: &[T], reference: f32) -> Option<f32> {
    samples
        .iter()
        .map(|s| libm::fabsf(s.to_f32() - reference))
        .reduce(f32::max)
}

/// Euclidean norm of a 3-vector.
pub fn magnitude(x: f32, y: f32, z: f32) -> f32 {
    libm::sqrtf(x * x + y * y + z * z)
}

/// Index of the first sample after the baseline that deviates from the
/// baseline mean by more than `k_sigma` baseline standard deviations.
///
/// The baseline is `samples[..baseline_len]`; the search covers
/// `samples[baseline_len..]` and the returned index is relative to the start
/// of `samples`.
///
/// Returns `None` if `baseline_len` is 0 or exceeds the slice length, or if no
/// sample crosses the threshold. With a perfectly flat baseline (σ = 0) the
/// threshold is 0, so the first sample that differs from the baseline at all
/// is the onset.
pub fn onset_index<T: Sample>(samples: &[T], baseline_len: usize, k_sigma: f32) -> Option<usize> {
    if baseline_len == 0 || baseline_len > samples.len() {
        return None;
    }
    let (baseline, rest) = samples.split_at(baseline_len);
    let m = mean(baseline)?;
    let threshold = k_sigma * std_dev(baseline)?;
    rest.iter()
        .position(|s| libm::fabsf(s.to_f32() - m) > threshold)
        .map(|i| i + baseline_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-4 * b.abs().max(1.0)
    }

    #[test]
    fn empty_slices_give_none() {
        let e: [i16; 0] = [];
        assert_eq!(mean(&e), None);
        assert_eq!(rms(&e), None);
        assert_eq!(std_dev(&e), None);
        assert_eq!(peak_abs(&e), None);
        assert_eq!(peak_abs_dev(&e, 0.0), None);
        assert_eq!(onset_index(&e, 0, 3.0), None);
    }

    #[test]
    fn mean_of_i16_and_f32() {
        assert!(close(mean(&[1i16, 2, 3, 4]).unwrap(), 2.5));
        assert!(close(mean(&[-1.5f32, 1.5, 3.0]).unwrap(), 1.0));
        assert!(close(mean(&[i16::MAX, i16::MAX]).unwrap(), 32767.0));
    }

    #[test]
    fn rms_about_zero() {
        assert!(close(rms(&[3i16, -4]).unwrap(), libm::sqrtf(12.5)));
        assert!(close(rms(&[2.0f32; 8]).unwrap(), 2.0));
        // No overflow for full-scale i16 input.
        assert!(close(rms(&[i16::MIN; 512]).unwrap(), 32768.0));
    }

    #[test]
    fn std_dev_removes_the_offset() {
        // Classic example: mean 5, population σ = 2.
        let d = [2i16, 4, 4, 4, 5, 5, 7, 9];
        assert!(close(mean(&d).unwrap(), 5.0));
        assert!(close(std_dev(&d).unwrap(), 2.0));
        // A large offset (1 g at ±16 g) does not disturb a small spread.
        let g = [2047i16, 2049, 2047, 2049];
        assert!(close(std_dev(&g).unwrap(), 1.0));
        assert!(close(std_dev(&[7i16]).unwrap(), 0.0));
    }

    #[test]
    fn peak_absolute_values() {
        assert!(close(peak_abs(&[1i16, -7, 5]).unwrap(), 7.0));
        assert!(close(peak_abs(&[i16::MIN, 3]).unwrap(), 32768.0));
        assert!(close(peak_abs(&[0.5f32, -0.25]).unwrap(), 0.5));
        assert!(close(
            peak_abs_dev(&[2048i16, 2100, 1900], 2048.0).unwrap(),
            148.0
        ));
    }

    #[test]
    fn vector_magnitude() {
        assert!(close(magnitude(3.0, 4.0, 0.0), 5.0));
        assert!(close(magnitude(1.0, 2.0, 2.0), 3.0));
        assert!(close(magnitude(0.0, 0.0, -2048.0), 2048.0));
    }

    #[test]
    fn onset_finds_the_first_crossing() {
        // Baseline: mean 0, σ = 1. Threshold at 3σ = 3.
        let s = [1i16, -1, 1, -1, 2, -3, 3, 4, 100, 0];
        assert_eq!(onset_index(&s, 4, 3.0), Some(7));
        // A lower threshold fires earlier; strictly greater than k·σ.
        assert_eq!(onset_index(&s, 4, 2.0), Some(5));
        assert_eq!(onset_index(&s, 4, 1.5), Some(4));
    }

    #[test]
    fn onset_detects_negative_excursions_and_offsets() {
        let s = [100.0f32, 102.0, 98.0, 100.0, 101.0, 60.0, 100.0];
        assert_eq!(onset_index(&s, 4, 5.0), Some(5));
    }

    #[test]
    fn onset_none_when_nothing_crosses() {
        let s = [1i16, -1, 1, -1, 1, -1, 2, -2];
        assert_eq!(onset_index(&s, 4, 5.0), None);
    }

    #[test]
    fn onset_rejects_bad_baseline_lengths() {
        let s = [1i16, 2, 3];
        assert_eq!(onset_index(&s, 0, 3.0), None);
        assert_eq!(onset_index(&s, 4, 3.0), None);
        // Baseline covering everything leaves nothing to search.
        assert_eq!(onset_index(&s, 3, 3.0), None);
    }

    #[test]
    fn onset_with_flat_baseline_uses_zero_threshold() {
        let s = [5i16, 5, 5, 5, 5, 6, 9];
        assert_eq!(onset_index(&s, 3, 4.0), Some(5));
    }
}
