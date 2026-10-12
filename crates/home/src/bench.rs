//! `stored` and `split`, for the bench targets only. Not a stable surface.

use types::channel;
use types::frame::key_set::KeySet;
use types::frame::{Draft, Frame};
use types::sample::Type;
use types::time::Stamp;

use crate::{split, stored};

/// Calls `stored::entry`.
pub fn entry(
    pool: &block::Pool,
    frame: &Frame,
    set: &KeySet,
    last: Stamp,
    stored_at: Stamp,
) -> Result<buffer::Entry, block::Error> {
    stored::entry(pool, frame, set, last, stored_at)
}

/// Calls `stored::read`, and gives each series' channel, type, and bytes.
pub fn read(body: &[u8]) -> impl Iterator<Item = (channel::Key, Type, &[u8])> {
    stored::read(body).map(|series| (series.channel, series.data_type, series.bytes))
}

/// The buffers of [`split`], which a shard keeps from one write to the next.
#[derive(Debug, Default)]
pub struct Scratch {
    split: split::Scratch,
    groups: Vec<u32>,
}

/// Checks `draft`, a writer's frame of `set`, and splits it as a shard's write does:
/// it encodes each raw series, walks the stamps of each group's index, and makes and
/// drops the index frame of each group. Returns the bytes of the series of those
/// frames.
///
/// # Panics
///
/// If `draft` is not of `set`, if a series does not fit its group's count, or if
/// `pool` has no block for an index frame.
pub fn split(
    scratch: &mut Scratch,
    pool: &block::Pool,
    set: &KeySet,
    draft: Draft,
) -> usize {
    let mut split = scratch.split.split(set, draft);
    scratch.groups.clear();
    while let Some((group, stamps)) = split.next() {
        let mut stamps = stamps.expect("each series fits its group's count");
        while let Some(vector) = stamps.next() {
            vector.expect("an index that decodes");
        }
        scratch.groups.push(group);
    }
    let mut bytes = 0;
    for &group in &scratch.groups {
        let mut frame = split.frame(pool, group).expect("room for an index frame");
        bytes += frame
            .iter_mut()
            .map(|(_, series)| series.len())
            .sum::<usize>();
    }
    bytes
}
