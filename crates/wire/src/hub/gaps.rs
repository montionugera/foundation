use std::ops;

use types::frame::Range;

/// The gap of each frame of a complete reader of an index: the seqs that it lost just
/// before the frame. The reader's node and the program each keep one for a session.
#[derive(Debug)]
pub struct Gaps {
    live: u64,
    last: ops::Range<u64>,
}

impl Gaps {
    /// The gaps of a reader that opened at `live`.
    #[must_use]
    pub fn new(live: u64) -> Self {
        Self { live, last: 0..0 }
    }

    /// Moves past the next frame, whose index has `range`. A frame with no sample
    /// moves nothing, so the next frame with one gets the whole gap. The caller gives
    /// the frames in order, each with a sample at or after [`Gaps::live`].
    ///
    /// # Panics
    ///
    /// When `range` ends past `u64::MAX`.
    #[inline]
    pub fn pass(&mut self, range: Range) {
        if range.count == 0 {
            self.last = 0..0;
            return;
        }
        self.last = if self.live < range.seq {
            self.live..range.seq
        } else {
            0..0
        };
        self.live = range.end().expect("a range ends at most at u64::MAX");
    }

    /// The seqs lost just before the last frame that [`Gaps::pass`] took: `0..0`
    /// when none, before the first frame, and when that frame had no sample.
    #[must_use]
    #[inline]
    pub fn last(&self) -> ops::Range<u64> {
        self.last.clone()
    }

    /// The first seq that the reader has not passed: the open position, then the end
    /// of the last frame with a sample.
    #[must_use]
    pub fn live(&self) -> u64 {
        self.live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(seq: u64, count: u32) -> Range {
        Range { seq, count }
    }

    /// Passes `range` and gives the gap before it.
    fn pass(gaps: &mut Gaps, range: Range) -> ops::Range<u64> {
        gaps.pass(range);
        gaps.last()
    }

    #[test]
    fn gives_no_gap_before_the_first_frame() {
        assert_eq!(Gaps::new(5).last(), 0..0);
    }

    #[test]
    fn gives_the_seqs_from_the_open_position_to_the_first_frame() {
        let mut gaps = Gaps::new(5);
        assert_eq!(pass(&mut gaps, range(9, 2)), 5..9);
        assert_eq!(gaps.live(), 11);
    }

    #[test]
    fn gives_no_gap_to_a_frame_at_the_end_of_the_last() {
        let mut gaps = Gaps::new(5);
        assert_eq!(pass(&mut gaps, range(5, 2)), 0..0);
        assert_eq!(pass(&mut gaps, range(7, 1)), 0..0);
        assert_eq!(gaps.live(), 8);
    }

    #[test]
    fn gives_the_gap_before_a_frame_with_no_sample_to_the_next_frame() {
        let mut gaps = Gaps::new(0);
        assert_eq!(pass(&mut gaps, range(4, 0)), 0..0);
        assert_eq!(gaps.live(), 0);
        assert_eq!(pass(&mut gaps, range(4, 1)), 0..4);
        assert_eq!(gaps.live(), 5);
    }

    #[test]
    fn gives_no_gap_to_a_frame_that_starts_before_the_live_seq() {
        let mut gaps = Gaps::new(5);
        assert_eq!(pass(&mut gaps, range(3, 4)), 0..0);
        assert_eq!(gaps.live(), 7);
    }

    #[test]
    #[should_panic(expected = "a range ends at most at u64::MAX")]
    fn panics_at_a_range_past_the_last_seq() {
        Gaps::new(0).pass(range(u64::MAX, 2));
    }
}
