//! Content digests.

use std::fmt;

/// The BLAKE3 hash of some bytes: the address of a spec chunk or a blob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// Hashes `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }
}

/// Computes the [`Digest`] of bytes that come in parts. The digest of the parts in order
/// is the digest of their concatenation.
#[derive(Clone, Debug, Default)]
pub struct Hasher(blake3::Hasher);

impl Hasher {
    /// Adds `bytes` after the bytes added before.
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    /// The digest of each byte added.
    #[must_use]
    pub fn finish(&self) -> Digest {
        Digest(*self.0.finalize().as_bytes())
    }
}

impl fmt::Display for Digest {
    /// Writes the digest as 64 lowercase hex digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{Digest, Hasher};

    #[test]
    fn hashes_with_blake3() {
        assert_eq!(
            Digest::of(b"").to_string(),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn differs_for_different_bytes() {
        assert_ne!(Digest::of(b"a"), Digest::of(b"b"));
    }

    proptest! {
        #[test]
        fn hashes_parts_as_their_concatenation(
            bytes in proptest::collection::vec(any::<u8>(), 0..4096),
            cuts in proptest::collection::vec(any::<usize>(), 0..8),
        ) {
            let mut cuts: Vec<usize> = cuts.iter().map(|cut| cut % (bytes.len() + 1)).collect();
            cuts.sort_unstable();
            let mut hasher = Hasher::default();
            let mut at = 0;
            for cut in cuts.into_iter().chain([bytes.len()]) {
                hasher.update(&bytes[at..cut]);
                at = cut;
            }
            prop_assert_eq!(hasher.finish(), Digest::of(&bytes));
        }
    }
}
