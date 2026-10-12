//! The limits of a node that a `node_settings` policy will set.

use std::num::{NonZeroU32, NonZeroUsize};

use types::time::Span;

/// The limits of a node's port, its peers, and its chunk store. `Default` gives each
/// value the node takes until a `node_settings` policy sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The transport's flow window of each stream and session, in bytes. It caps a
    /// session at the window over the round trip. At least twice the message limit,
    /// else the start stops with [`crate::Error::Transport`]. Default 1 MiB.
    pub window_bytes: usize,
    /// The most streams of each kind a peer may have open at once, per session. A
    /// peer that opens one more waits for credit. Default 64.
    pub streams_max: NonZeroU32,
    /// How long a silent peer keeps its session. Positive, else the start stops with
    /// [`crate::Error::Transport`]. Default 30 s.
    pub idle: Span,
    /// The largest message of a stream. Each shard takes the smaller of this and the
    /// largest block of its pool, which must be at least 1472 bytes, else the start
    /// stops with [`crate::Error::Transport`]. Default 64 KiB.
    pub message_bytes_max: NonZeroUsize,
    /// How long a stream may take to give its header before it is rejected. Default
    /// 10 s.
    pub header: Span,
    /// The most places the peers outside the region hold at once. A program holds one
    /// for each session, and a node one for its key. Default 256.
    pub places_max: NonZeroUsize,
    /// The bytes the chunk store leaves free on the disk. Default 1 GiB.
    pub floor_bytes: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            window_bytes: 1 << 20,
            streams_max: NonZeroU32::new(64).expect("not zero"),
            idle: Span::from_nanos(30 * Span::SECOND.nanos()),
            message_bytes_max: NonZeroUsize::new(1 << 16).expect("not zero"),
            header: Span::from_nanos(10 * Span::SECOND.nanos()),
            places_max: NonZeroUsize::new(256).expect("not zero"),
            floor_bytes: 1 << 30,
        }
    }
}
