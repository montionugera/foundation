//! The deadlines of a commit task with no file, for [`crate::bench`].

use std::num::NonZeroU32;

use block::{Block, Heap, Pool};
use types::channel::{self, Slot};
use types::frame::Path;
use types::time::Stamp;

use super::{HEADER_LEN, State, seal, take};
use crate::carry;
use crate::entry::{Entry, Samples};
use crate::group::{self, Closed, Sealed};
use crate::header::Header;
use crate::log::Logs;
use crate::record::{ALIGN, BLOCK};
use crate::wal::{Layout, Writer};

/// The commit task of a buffer with [`Config::trimmed`](super::Config::trimmed), with
/// no file, over a ring that opened empty.
#[derive(Debug)]
pub struct Deadline {
    state: State,
    pool: Pool,
    block_max: usize,
    tails_max: usize,
    part: Block,
    /// The next seq of each index.
    firsts: Vec<u64>,
    batch: Vec<Entry>,
    taken: Vec<Closed>,
    sealed: Vec<Sealed>,
    chain: u32,
    deadlines: u32,
}

impl Deadline {
    /// The task of a ring of `blocks` blocks of 4096 bytes, with bodies of at most two
    /// blocks, over `indexes` indexes on the live path, and a pool of 2 MiB, as
    /// [`Group`](crate::bench::Group) has.
    ///
    /// # Panics
    ///
    /// When `blocks` makes no valid layout.
    #[must_use]
    pub fn new(indexes: u32, blocks: u64) -> Self {
        let area = blocks.checked_mul(BLOCK).expect("a valid layout");
        let layout = Layout::new(area, 2 * ALIGN).expect("a valid layout");
        let config = block::Config::new(1 << 21).expect("the budget fits");
        let pool = Pool::new(config.clone(), Heap::new(config.reservation()));
        let block_max = group::block_max(&pool).expect("the budget holds an entry");
        let tails_max = carry::count(pool.largest().saturating_sub(HEADER_LEN));
        let part = pool.alloc(100).expect("the pool has room").freeze();
        let chain = 9;
        let state = State::new(
            Writer::empty(layout, 0),
            Logs::new(area),
            Header::new(layout, chain),
        );
        Self {
            state,
            pool,
            block_max,
            tails_max,
            part,
            firsts: vec![0; usize::try_from(indexes).expect("fits")],
            batch: Vec::new(),
            taken: Vec::new(),
            sealed: Vec::new(),
            chain,
            deadlines: 0,
        }
    }

    /// One deadline after the append of one data entry of 100 bytes of each index
    /// but the last, which appends one each `period` deadlines: the take of the
    /// group, the checkpoint of the trim and the carry record, the seal of the
    /// records, and their sync, with no file write. Gives the tails of the carry
    /// record.
    ///
    /// # Panics
    ///
    /// When the ring or the pool has no room for the group.
    pub fn commit(&mut self, period: u32) -> usize {
        let slow = self.deadlines.is_multiple_of(period);
        self.deadlines = self.deadlines.wrapping_add(1);
        let appended = self.firsts.len() - usize::from(!slow);
        let part = &self.part;
        let entries = self.firsts.iter_mut().take(appended).zip(0..);
        self.batch.extend(entries.map(|(first, n)| {
            let entry = Entry {
                index: channel::Key::from_u128(n.into()),
                slot: Slot::new(n),
                path: Path::Live,
                first: *first,
                samples: Some(Samples {
                    len: NonZeroU32::MIN,
                    last: Some(Stamp::from_nanos(9)),
                }),
                stored_at: Stamp::from_nanos(7),
                tag: 0,
                parts: part.clone().into(),
            };
            *first += 1;
            entry
        }));
        let state = &mut self.state;
        state
            .open
            .fit(&self.pool, self.block_max, &state.writer, &self.batch)
            .expect("the ring has room");
        take(&mut state.open, &mut state.logs, &mut self.batch);
        state.take(&mut self.taken);
        let checkpoint = state.checkpoint(&self.pool);
        let carry = state.carry(&self.pool, self.tails_max);
        let carrying;
        (carrying, self.chain) =
            seal(&mut self.taken, &mut self.sealed, carry, self.chain);
        let tails = carrying
            .as_ref()
            .map_or(0, |carrying| carrying.carried.tails().len());
        let tail = checkpoint.map(|checkpoint| checkpoint.tail);
        state.synced(&mut self.sealed, carrying, tail);
        tails
    }
}
