//! The state entries of an index: entries of no samples on its live path, each with
//! the tag of its kind, where the newest of a kind replaces the ones before it.

use std::num::NonZeroU8;

use block::Block;
use buffer::Entry;
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
        len: 0,
        stored_at,
        last: None,
        tag: tag.get(),
        parts: body.into(),
    }
}
