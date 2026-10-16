//! The state entries of an index: entries of no samples on its live path, each with
//! the tag of its kind, where the newest of a kind replaces the ones before it.

use std::num::NonZeroU8;

use block::Block;
use buffer::{Buffer, Entry};
use types::channel::{self, Slot};
use types::frame::Path;
use types::time::Stamp;

/// The state entry of tag `tag` with `body`, or no part for `None`, on the live path
/// of the index `index` at `slot`, at seq `first` and mesh time `stored_at`.
pub(crate) fn entry(
    tag: NonZeroU8,
    body: Option<Block>,
    index: channel::Key,
    slot: Slot,
    first: u64,
    stored_at: Stamp,
) -> Entry {
    Entry {
        index,
        slot,
        path: Path::Live,
        first,
        samples: None,
        stored_at,
        tag: tag.get(),
        parts: body.into(),
    }
}

/// Appends `entry`, a state entry whose body a block of the pool of `buffer` holds,
/// alone. Returns whether the entry needs no later append: the buffer took it, or a
/// failed commit ended the buffer, so a restart reads no later entry. `false` when
/// the pool had no block for the body, or the ring or the pool had no room.
///
/// # Panics
///
/// If the body passes the most bytes of a block or of one entry of the ring. Each
/// kind keeps its bodies within them.
pub(crate) fn append(buffer: &Buffer, entry: Result<Entry, block::Error>) -> bool {
    let entry = match entry {
        Ok(entry) => entry,
        Err(block::Error::Exhausted { .. } | block::Error::Refused { .. }) => {
            return false;
        }
        Err(error @ block::Error::TooLarge { .. }) => {
            panic!("invariant: a block holds the body of a state entry: {error}")
        }
    };
    match buffer.append([entry]) {
        Ok(()) | Err(buffer::Rejected::Files(_)) => true,
        Err(buffer::Rejected::Full { .. } | buffer::Rejected::Pool(_)) => false,
        Err(buffer::Rejected::Large(limit)) => {
            panic!("invariant: one entry holds the body of a state entry: {limit}")
        }
    }
}
