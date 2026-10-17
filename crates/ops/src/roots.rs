//! The failures of the read of the OS roots of a node, which `status` gives.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::escape_controls;

#[cfg(test)]
mod tests;

/// The most bytes that the source and the failures of [`Failures`] hold, so that they
/// fit in the reply of `status` also with each byte escaped.
const KEPT: usize = 1 << 20;

/// The failures of the read of the OS roots of a node.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(from = "Wire")]
pub struct Failures {
    /// Where the read looked.
    source: String,
    /// The text of each failure, in the order of the read.
    #[serde(rename = "failures")]
    listed: Vec<String>,
    /// The count of the failures after `listed`, which the node does not keep.
    unlisted: u64,
}

/// The fields of [`Failures`] as a reply gives them, before [`Failures::new`] bounds
/// them. The schema of [`Failures`] takes the docs of its fields from here.
#[derive(Deserialize, JsonSchema)]
struct Wire {
    /// Where the read looked.
    source: String,
    /// The text of each failure, in the order of the read.
    failures: Vec<String>,
    /// The count of the failures after `failures`, which the node does not keep.
    unlisted: u64,
}

impl From<Wire> for Failures {
    fn from(wire: Wire) -> Self {
        let mut failures = Self::new(wire.source, wire.failures);
        failures.unlisted = failures.unlisted.saturating_add(wire.unlisted);
        failures
    }
}

impl Failures {
    /// The `failures` of a read of the OS roots from `source`, in the order of the
    /// read. Keeps `source` in full, and each failure in full, in order, while the
    /// memory of `source` and the kept failures is at most 1 MiB, and from the first
    /// that does not fit only counts them, so that the reply of `status` stays in its
    /// bound. A `source` of more than 1 MiB keeps no failure.
    #[must_use]
    pub fn new(source: String, mut failures: Vec<String>) -> Self {
        let mut held = source.capacity();
        let kept = failures
            .iter()
            .take_while(|failure| {
                held += size_of::<String>() + failure.capacity();
                held <= KEPT
            })
            .count();
        let unlisted = count(failures.len() - kept);
        failures.truncate(kept);
        failures.shrink_to_fit();
        Self {
            source,
            listed: failures,
            unlisted,
        }
    }

    /// The lines of these failures in `status::Output::text`, or `None` when the read
    /// had no failure.
    pub(crate) fn lines(&self) -> Option<String> {
        if self.listed.is_empty() && self.unlisted == 0 {
            return None;
        }
        let failed = count(self.listed.len()).saturating_add(self.unlisted);
        let source = escape_controls(&self.source);
        let head = format!("OS roots: {failed} failures, from {source}\n");
        let errors = (self.listed.iter())
            .map(|failure| format!("OS roots: error: {}\n", escape_controls(failure)));
        let unlisted = (self.unlisted > 0)
            .then(|| format!("OS roots: left out: {}\n", self.unlisted));
        let lines = std::iter::once(head).chain(errors).chain(unlisted);
        Some(lines.collect())
    }
}

/// `len` as a count of the wire.
fn count(len: usize) -> u64 {
    u64::try_from(len).expect("invariant: a length fits in 64 bits")
}
