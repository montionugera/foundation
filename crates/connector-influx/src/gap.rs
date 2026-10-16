//! Gap lines: one line for each run of samples that the reader did not get, as the
//! buffer trimmed them or had no room for them.

use std::ops::Range;

use types::frame::Path;
use types::name::Name;
use types::time::Stamp;

use crate::line::{Field, Measurement, Value};

/// The measurement of the gap lines.
pub const MEASUREMENT: &str = "foundation_gaps";

/// The gap of one path of one index of one connector. It holds the seqs from the first
/// trimmed seq up to the next sample, then writes them as one line at that sample's
/// stamp.
#[derive(Debug)]
pub struct Gap {
    measurement: Measurement,
    count: Field,
    seqs: Option<Range<u64>>,
}

impl Gap {
    /// An empty gap of one path, with `connector`, `index`, and `path` (`live` or
    /// `backfill`) as its tags.
    #[must_use]
    #[expect(
        clippy::missing_panics_doc,
        reason = "a name or a path is a valid tag value"
    )]
    pub fn new(connector: &Name, index: &Name, path: Path) -> Self {
        let path = match path {
            Path::Live => "live",
            Path::Backfill => "backfill",
        };
        let tags = [
            ("connector", connector.as_str()),
            ("index", index.as_str()),
            ("path", path),
        ];
        let measurement = Measurement::new(MEASUREMENT, &tags)
            .expect("invariant: a name or a path is a valid tag value");
        Self {
            count: measurement
                .field("count")
                .expect("invariant: a valid field key"),
            measurement,
            seqs: None,
        }
    }

    /// Adds the trimmed samples with the seqs in `seqs`. The gap then holds each seq
    /// from the least start it got, also the seqs between two ranges that do not touch.
    /// A seq that the gap already holds is held once, so a gap that the reader reports
    /// again counts once. A range with `start == end` adds nothing.
    ///
    /// # Panics
    ///
    /// When `seqs` is reversed: `start > end`.
    pub fn add(&mut self, seqs: Range<u64>) {
        assert!(
            seqs.start <= seqs.end,
            "invariant: a gap range is not reversed, got {}..{}",
            seqs.start,
            seqs.end
        );
        if seqs.is_empty() {
            return;
        }
        self.seqs = Some(match self.seqs.take() {
            Some(held) => held.start.min(seqs.start)..held.end.max(seqs.end),
            None => seqs,
        });
    }

    /// Appends the gap's line to `out`, empties the gap, and gives the line's count.
    /// The line counts each seq from the gap's first seq up to `seq`, the seq of the
    /// first sample after the gap, and has that sample's `stamp`. An empty gap appends
    /// nothing and gives 0.
    ///
    /// Only `out` then holds the gap: a request built again from the samples holds
    /// it only when the reader reports the gap again.
    ///
    /// # Panics
    ///
    /// When `seq` is below the end of a range the gap holds, or the line counts
    /// 2^63 seqs or more.
    pub fn line(&mut self, out: &mut Vec<u8>, seq: u64, stamp: Stamp) -> u64 {
        let Some(seqs) = self.seqs.take() else {
            return 0;
        };
        assert!(
            seqs.end <= seq,
            "invariant: the sample after the gap {seqs:?} has seq {seq}"
        );
        let count = seq
            .checked_sub(seqs.start)
            .and_then(|count| i64::try_from(count).ok())
            .unwrap_or_else(|| {
                panic!(
                    "invariant: a gap counts fewer than 2^63 seqs, from {} to {seq}",
                    seqs.start
                )
            });
        let values = [(&self.count, Value::Integer(count))];
        self.measurement.line(out, values, stamp);
        count.unsigned_abs()
    }
}

#[cfg(test)]
mod tests;
