//! Parser for headerless accelerometer FIFO data.
//!
//! In headerless mode with only the accelerometer enabled, the BMI160 FIFO is
//! a plain sequence of 6-byte frames: X, Y, Z as little-endian `i16`
//! (datasheet §2.5.1). [`frames`] iterates over the whole frames in a byte
//! slice and exposes whatever is left over as a trailing partial frame.
//!
//! # Over-reading
//!
//! Reading more bytes than the FIFO holds returns invalid frames that "always
//! contain the fixed expression (magic number) 0x80" (datasheet §2.5.1).
//! [`is_overread_marker`] flags frames whose X value is −32768 (bytes `00 80`).
//!
// UNVERIFIED: the datasheet does not say where in the frame the 0x80 appears;
// "X reads 0x8000" is the interpretation used by Bosch's reference driver, as
// far as remembered, and has not been observed on hardware yet.
//!
//! A real sample saturated at −16 g looks the same, so the parser never drops
//! frames on its own. Bound the read by FIFO_LENGTH instead
//! ([`crate::bmi160::Bmi160::read_fifo_frames`]).

use crate::Accel;

/// Length of one headerless accelerometer frame in bytes.
pub const FRAME_LEN: usize = Accel::WIRE_LEN;

/// Iterate over the whole 6-byte frames in `bytes`.
///
/// A trailing partial frame (1–5 bytes) is not yielded; it stays available
/// through [`Frames::remainder`].
pub fn frames(bytes: &[u8]) -> Frames<'_> {
    let (chunks, remainder) = bytes.as_chunks::<FRAME_LEN>();
    Frames {
        chunks: chunks.iter(),
        remainder,
    }
}

/// Number of whole frames in a buffer of `len` bytes.
pub const fn frame_count(len: usize) -> usize {
    len / FRAME_LEN
}

/// True if `sample` has the X value produced by over-reading the FIFO
/// (0x8000). See the module docs for why this is only a hint.
pub const fn is_overread_marker(sample: Accel) -> bool {
    sample.x == i16::MIN
}

/// Iterator over headerless accelerometer frames; created by [`frames`].
#[derive(Debug, Clone)]
pub struct Frames<'a> {
    chunks: core::slice::Iter<'a, [u8; FRAME_LEN]>,
    remainder: &'a [u8],
}

impl<'a> Frames<'a> {
    /// The trailing bytes that do not form a whole frame (empty, or 1–5 bytes).
    ///
    /// Independent of how far the iterator has advanced.
    pub fn remainder(&self) -> &'a [u8] {
        self.remainder
    }
}

impl Iterator for Frames<'_> {
    type Item = Accel;

    fn next(&mut self) -> Option<Accel> {
        self.chunks.next().map(|c| Accel::from_le_bytes(*c))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.chunks.size_hint()
    }
}

impl ExactSizeIterator for Frames<'_> {}

impl DoubleEndedIterator for Frames<'_> {
    fn next_back(&mut self) -> Option<Accel> {
        self.chunks.next_back().map(|c| Accel::from_le_bytes(*c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slice_yields_nothing() {
        let mut it = frames(&[]);
        assert_eq!(it.len(), 0);
        assert!(it.next().is_none());
        assert!(it.remainder().is_empty());
    }

    #[test]
    fn parses_whole_frames_in_order() {
        let bytes = [
            0x01, 0x00, 0x02, 0x00, 0x03, 0x00, // (1, 2, 3)
            0xFF, 0xFF, 0x00, 0x80, 0xFF, 0x7F, // (-1, MIN, MAX)
        ];
        let v: Vec<Accel> = frames(&bytes).collect();
        assert_eq!(
            v,
            vec![Accel::new(1, 2, 3), Accel::new(-1, i16::MIN, i16::MAX)]
        );
        assert!(frames(&bytes).remainder().is_empty());
        assert_eq!(frame_count(bytes.len()), 2);
    }

    #[test]
    fn trailing_partial_frame_is_reported_not_parsed() {
        let bytes = [1, 0, 2, 0, 3, 0, 0xAA, 0xBB, 0xCC];
        let mut it = frames(&bytes);
        assert_eq!(it.len(), 1);
        assert_eq!(it.remainder(), &[0xAA, 0xBB, 0xCC]);
        assert_eq!(it.next(), Some(Accel::new(1, 2, 3)));
        assert_eq!(it.next(), None);
        // Still available after the iterator is exhausted.
        assert_eq!(it.remainder(), &[0xAA, 0xBB, 0xCC]);
        assert_eq!(frame_count(bytes.len()), 1);
    }

    #[test]
    fn fewer_than_six_bytes_is_only_a_remainder() {
        let bytes = [9, 8, 7, 6, 5];
        let mut it = frames(&bytes);
        assert!(it.next().is_none());
        assert_eq!(it.remainder(), &bytes);
    }

    #[test]
    fn iterates_from_the_back() {
        let bytes = [1, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 0, 0xEE];
        let xs: Vec<i16> = frames(&bytes).rev().map(|a| a.x).collect();
        assert_eq!(xs, vec![3, 2, 1]);
    }

    #[test]
    fn overread_marker() {
        // The assumed over-read pattern: 0x8000 in every axis.
        let bytes = [0x00, 0x80, 0x00, 0x80, 0x00, 0x80];
        let a = frames(&bytes).next().unwrap();
        assert!(is_overread_marker(a));
        assert!(!is_overread_marker(Accel::new(-32767, 0, 0)));
    }
}
