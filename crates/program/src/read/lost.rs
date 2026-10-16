//! The seqs that each entry of a complete reader lost since its last series.

use std::ops::Range;

/// The lost seqs of each entry of a reader's key set, as the frames pass.
#[derive(Debug)]
pub(super) struct Lost {
    entries: Vec<Entry>,
    /// The count of frames passed.
    passed: u64,
    /// The frame, of `passed`, of the last gap.
    gapped: u64,
    /// The count of entries with no series since the last gap.
    pending: usize,
}

/// The lost seqs of one entry.
#[derive(Clone, Debug, Default)]
struct Entry {
    /// While `at` is before the last gap, the seqs lost since the series at `at`,
    /// which the next series of the entry gets. Else the seqs that the series at `at`
    /// got, stale once a later frame passes.
    seqs: Range<u64>,
    /// The frame, of `passed`, of the last series of the entry. While none is pending,
    /// it may stay at an earlier series at or after the last gap.
    at: u64,
}

impl Lost {
    /// The lost seqs of a reader of `entries` entries, before its first frame.
    pub(super) fn new(entries: usize) -> Self {
        Self {
            entries: vec![Entry::default(); entries],
            passed: 0,
            gapped: 0,
            pending: 0,
        }
    }

    /// Moves past the next frame, which lost `gap` and holds a series with a sample
    /// of each entry of `held`. Touches each entry at a gap, else only those of
    /// `held`, and none while each entry got a series since the last gap.
    ///
    /// # Panics
    ///
    /// When an entry of `held` is not below the count of entries.
    pub(super) fn pass(
        &mut self,
        gap: Range<u64>,
        held: impl IntoIterator<Item = usize>,
    ) {
        self.passed += 1;
        if !gap.is_empty() {
            for entry in &mut self.entries {
                entry.seqs = if entry.at >= self.gapped {
                    gap.clone()
                } else {
                    entry.seqs.start..gap.end
                };
            }
            self.gapped = self.passed;
            self.pending = self.entries.len();
        }
        // With none pending, each `at` is at or after `gapped` and stays so, and
        // `get` gives `0..0`, so a stale `at` changes no result.
        if self.pending == 0 {
            return;
        }
        for entry in held {
            let entry = &mut self.entries[entry];
            if entry.at >= self.gapped {
                entry.seqs = 0..0;
            } else {
                self.pending -= 1;
            }
            entry.at = self.passed;
        }
    }

    /// The seqs lost after the last series of `entry` with a sample up to the last
    /// frame, from the first lost seq to the last. `0..0` when that frame holds no
    /// sample of `entry`, when none was lost, and before the first frame.
    ///
    /// # Panics
    ///
    /// When `entry` is not below the count of entries.
    pub(super) fn get(&self, entry: usize) -> Range<u64> {
        let entry = &self.entries[entry];
        if entry.at == self.passed {
            entry.seqs.clone()
        } else {
            0..0
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const ENTRIES: usize = 4;

    /// The gap and the held entries of each frame: a gap of up to 3 seqs before about
    /// one frame in three, and a random set of entries.
    fn frames() -> impl Strategy<Value = Vec<(Range<u64>, Vec<usize>)>> {
        let frame = (prop_oneof![2 => Just(0_u64), 1 => 1_u64..4], 0_u8..16);
        prop::collection::vec(frame, 0..24).prop_map(|frames| {
            let mut next = 0;
            frames
                .into_iter()
                .map(|(jump, set)| {
                    let gap = if jump == 0 { 0..0 } else { next..next + jump };
                    next += jump + 1;
                    let held = (0..ENTRIES).filter(|&e| set & (1 << e) != 0).collect();
                    (gap, held)
                })
                .collect()
        })
    }

    #[test]
    fn gives_nothing_before_the_first_frame() {
        let lost = Lost::new(2);
        assert_eq!([lost.get(0), lost.get(1)], [0..0, 0..0]);
    }

    #[test]
    fn gives_an_entry_the_gaps_since_its_last_series_as_one_range() {
        let mut lost = Lost::new(2);
        lost.pass(0..0, [0, 1]);
        lost.pass(1..3, [0]);
        assert_eq!([lost.get(0), lost.get(1)], [1..3, 0..0]);
        lost.pass(4..6, [0]);
        assert_eq!([lost.get(0), lost.get(1)], [4..6, 0..0]);
        lost.pass(0..0, [0, 1]);
        assert_eq!([lost.get(0), lost.get(1)], [0..0, 1..6]);
    }

    #[test]
    fn gives_the_last_entry_its_gap_after_the_others_had_a_series() {
        let mut lost = Lost::new(3);
        lost.pass(1..3, [0]);
        assert_eq!([lost.get(0), lost.get(1), lost.get(2)], [1..3, 0..0, 0..0]);
        lost.pass(0..0, [0, 1]);
        assert_eq!([lost.get(0), lost.get(1), lost.get(2)], [0..0, 1..3, 0..0]);
        lost.pass(0..0, [2]);
        assert_eq!([lost.get(0), lost.get(1), lost.get(2)], [0..0, 0..0, 1..3]);
        lost.pass(0..0, [0, 1, 2]);
        assert_eq!([lost.get(0), lost.get(1), lost.get(2)], [0..0, 0..0, 0..0]);
    }

    #[test]
    fn reads_no_held_entry_once_each_got_a_series_after_the_gap() {
        let read = std::cell::Cell::new(0);
        let counted = |held: &'static [usize]| {
            held.iter().copied().inspect(|_| read.set(read.get() + 1))
        };
        let mut lost = Lost::new(2);
        lost.pass(0..0, counted(&[0, 1]));
        assert_eq!(read.get(), 0);
        lost.pass(1..3, counted(&[0]));
        lost.pass(0..0, counted(&[1]));
        assert_eq!(read.get(), 2);
        lost.pass(0..0, counted(&[0, 1]));
        assert_eq!(read.get(), 2);
    }

    #[test]
    #[should_panic(expected = "index out of bounds")]
    fn panics_on_an_entry_past_the_last() {
        let _lost = Lost::new(2).get(2);
    }

    proptest! {
        /// After each frame, an entry that the frame holds gets the span of the gaps
        /// since its last series, and any other entry gets `0..0`.
        #[test]
        fn gives_each_entry_the_span_of_its_gaps(frames in frames()) {
            let mut lost = Lost::new(ENTRIES);
            let mut since: [Option<Range<u64>>; ENTRIES] = Default::default();
            for (gap, held) in frames {
                lost.pass(gap.clone(), held.iter().copied());
                for (entry, span) in since.iter_mut().enumerate() {
                    if !gap.is_empty() {
                        *span = Some(span.take().map_or(gap.clone(), |s| s.start..gap.end));
                    }
                    if held.contains(&entry) {
                        prop_assert_eq!(lost.get(entry), span.take().unwrap_or(0..0));
                    } else {
                        prop_assert_eq!(lost.get(entry), 0..0);
                    }
                }
            }
        }
    }
}
