//! Where a read of one path of an index's log stands.

/// Where a read of a path stands: before the entries that end past `seq`, and past the
/// first `given` entries with no samples at `seq`. Marks order as reads do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Mark {
    /// The seq that the entries the read has not given end past.
    pub seq: u64,
    /// How many entries with no samples at `seq` the read gave.
    pub given: u64,
}

impl Mark {
    /// The mark before every entry that ends past `seq`.
    #[must_use]
    pub const fn at(seq: u64) -> Self {
        Self { seq, given: 0 }
    }
}
