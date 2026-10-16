//! The CA certificates of the OS store.

/// The CA certificates of the OS store, and each failure of the read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    /// The DER bytes of each certificate that the read found.
    pub certificates: Vec<Vec<u8>>,
    /// The text of each failure. A failure skips its part of the store, and the read
    /// goes on.
    pub failures: Vec<String>,
}
