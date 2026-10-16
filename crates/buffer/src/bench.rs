//! `wal::Writer`, `log::Logs`, and a header table, for the bench targets and the
//! `alloc` test only. Not a stable surface.

use std::{hint, iter, mem};

use block::{Block, Heap, Pool};
use types::channel::{self, Slot};
use types::frame::Path;
use types::time::Stamp;

use crate::entry::{self, Entry, Header, Last};
use crate::group;
use crate::log::Carried;
use crate::record::{ALIGN, BLOCK};
use crate::wal::{Ends, Layout, Position, Writer};

/// The blocks that each ring of the `wal` bench adds to its ID. With the three
/// blocks that each record keeps free, each commit of 1 record then trims at 68 and
/// 4100 blocks.
#[doc(hidden)]
pub const PHASE: u64 = 4;

/// The area of the ring of [`Logs`]: 256 records of a block.
const AREA: u64 = 256 * BLOCK;

/// The body lengths of the records of a commit, in turn.
const LENS: [usize; 5] = [8, 8, ALIGN, 8, 2 * ALIGN];

/// A write-ahead ring with no file: the writer of a ring that opened empty.
#[derive(Debug)]
pub struct Ring {
    writer: Writer,
    ends: Vec<Ends>,
    /// The writer does not check chain values, so a counter stands in for them.
    chain: u32,
    next: usize,
}

impl Ring {
    /// An empty ring of `blocks` blocks of 4096 bytes, with bodies of at most two
    /// blocks.
    ///
    /// # Panics
    ///
    /// When `blocks` makes no valid layout.
    #[must_use]
    pub fn new(blocks: u64) -> Self {
        let area = blocks.checked_mul(BLOCK).expect("a valid layout");
        let layout = Layout::new(area, 2 * ALIGN).expect("a valid layout");
        Self {
            writer: Writer::empty(layout, 0),
            ends: Vec::new(),
            chain: 9,
            next: 0,
        }
    }

    /// One commit of `records` records, with bodies of 8 bytes, 8 bytes, one block, 8
    /// bytes, and two blocks in turn: `append` for each, then `trimmed`, and
    /// `committed` with the records and the trimmed tail.
    ///
    /// # Panics
    ///
    /// When the ring has no room for a record.
    pub fn commit(&mut self, records: usize) {
        for _ in 0..records {
            let len = LENS[self.next % LENS.len()];
            self.next += 1;
            let plan = self.writer.append(len).expect("the ring has room");
            let wrap = plan.wrap.map(|_| {
                self.chain = self.chain.wrapping_add(1);
                Position::new(plan.offset, self.chain).expect("aligned")
            });
            self.chain = self.chain.wrapping_add(1);
            let record = Position::new(plan.next, self.chain).expect("aligned");
            self.ends.push(Ends { wrap, record });
        }
        let tail = self.writer.trimmed(None);
        self.writer.committed(self.ends.drain(..), tail);
    }
}

/// A group commit with no file: the groups of a buffer over the writer of a ring that
/// opened empty.
#[derive(Debug)]
pub struct Group {
    writer: Writer,
    pool: Pool,
    block_max: usize,
    /// The bytes of each entry.
    part: Block,
    /// The cleared group of the last commit, with its capacity.
    spare: group::Group,
    batch: Vec<Entry>,
    chain: u32,
}

impl Group {
    /// An empty ring of `blocks` blocks of 4096 bytes, with bodies of at most two
    /// blocks, and a pool for the group headers.
    ///
    /// # Panics
    ///
    /// When `blocks` makes no valid layout.
    #[must_use]
    pub fn new(blocks: u64) -> Self {
        let area = blocks.checked_mul(BLOCK).expect("a valid layout");
        let layout = Layout::new(area, 2 * ALIGN).expect("a valid layout");
        let config = block::Config::new(1 << 21).expect("the budget fits");
        let pool = Pool::new(config.clone(), Heap::new(config.reservation()));
        let block_max = group::block_max(&pool).expect("the budget holds an entry");
        let part = pool.alloc(100).expect("the pool has room").freeze();
        Self {
            writer: Writer::empty(layout, 0),
            pool,
            block_max,
            part,
            spare: group::Group::default(),
            batch: Vec::new(),
            chain: 9,
        }
    }

    /// One commit of one group of `entries` entries of 100 bytes each: the push of
    /// the entries, which checks that the record fits, then the close, the seal, the
    /// trim, and the sync of the record. The time also holds the bench's own work to
    /// make the entries.
    ///
    /// # Panics
    ///
    /// When the ring or the pool has no room for the group.
    pub fn commit(&mut self, entries: usize) {
        let entry = Entry {
            index: channel::Key::from_u128(1),
            slot: Slot::new(0),
            path: Path::Live,
            first: 0,
            len: 1,
            stored_at: Stamp::from_nanos(7),
            last: Some(Stamp::from_nanos(9)),
            tag: 0,
            parts: self.part.clone().into(),
        };
        self.batch.extend(iter::repeat_n(entry, entries));
        let mut group = mem::take(&mut self.spare);
        group
            .push(&self.pool, self.block_max, &self.writer, &mut self.batch)
            .expect("the ring has room");
        let sealed = group.close(&mut self.writer).seal(self.chain);
        self.chain = sealed.ends().record.chain();
        let tail = self.writer.trimmed(None);
        self.writer.committed([sealed.ends()], tail);
        self.spare = sealed.clear();
    }
}

/// The durable logs of a shard, synced as each commit syncs them.
#[derive(Debug)]
pub struct Logs {
    inner: crate::log::Logs,
    headers: Vec<(Slot, Header)>,
    offset: u64,
    commits: u32,
    carried: Carried,
}

impl Logs {
    /// The logs of `indexes` indexes on the live path.
    #[must_use]
    pub fn new(indexes: u32) -> Self {
        let headers = (0..indexes)
            .map(|n| {
                let header = Header {
                    index: channel::Key::from_u128(u128::from(n) + 1),
                    path: Path::Live,
                    first: 0,
                    len: 1,
                    stored_at: Stamp::from_nanos(1),
                    last: Last::Stamp(Some(Stamp::from_nanos(1))),
                    tag: 0,
                    bytes: 8,
                };
                (Slot::new(n), header)
            })
            .collect();
        Self {
            inner: crate::log::Logs::new(AREA),
            headers,
            offset: BLOCK,
            commits: 0,
            carried: Carried::with_capacity(usize::try_from(indexes).expect("fits")),
        }
    }

    /// One commit of one record that holds `entries` data entries of each index. It
    /// first hides the records before the oldest holder, as a trim that keeps up, so
    /// each log holds the run of its holder. Then it syncs each entry.
    pub fn commit(&mut self, entries: u32) {
        self.keep_up();
        for (slot, header) in &mut self.headers {
            for _ in 0..entries {
                self.inner
                    .sync(*slot, header, self.offset)
                    .expect("entries in order");
                header.first += 1;
            }
        }
        self.offset += BLOCK;
    }

    /// One commit as `commit(1)`, with an append of each entry, then a carry record
    /// in the next record, as a commit that carries: it takes the idle tails before
    /// the record and makes the record their holder, so the hide of the next commit
    /// drops the run of each index.
    pub fn carry(&mut self) {
        self.keep_up();
        for (slot, header) in &mut self.headers {
            self.inner.append(*slot, header).expect("entries in order");
            self.inner
                .sync(*slot, header, self.offset)
                .expect("entries in order");
            header.first += 1;
        }
        self.offset += BLOCK;
        self.carried.clear();
        self.inner
            .idle(self.offset, self.headers.len(), &mut self.carried);
        self.inner.hold(&self.carried, self.offset);
        self.offset += BLOCK;
    }

    /// Moves the appended tail of each index past one data entry, with no sync.
    pub fn append(&mut self) {
        for (slot, header) in &mut self.headers {
            self.inner.append(*slot, header).expect("entries in order");
            header.first += 1;
        }
    }

    /// One commit that the oldest holder bounds, as a commit that trims. It appends
    /// and syncs one data entry of each index but the last, which syncs one each
    /// `period` commits, so its item is the last of its record. Then it takes the
    /// idle tails before the record, and when one is idle, the other tails of the
    /// record, at most one per index. It hides the records before the oldest
    /// holder, or before the record `held` commits back, as a read that holds it,
    /// when that one is older.
    pub fn trim(&mut self, period: u32, held: u64) {
        let slow = self.commits.is_multiple_of(period);
        self.commits = self.commits.wrapping_add(1);
        let synced = self.headers.len() - usize::from(!slow);
        for (slot, header) in self.headers.iter_mut().take(synced) {
            self.inner.append(*slot, header).expect("entries in order");
            self.inner
                .sync(*slot, header, self.offset)
                .expect("entries in order");
            header.first += 1;
        }
        self.carried.clear();
        self.inner
            .idle(self.offset, self.headers.len(), &mut self.carried);
        let kept = self.inner.kept().expect("a path has a synced entry");
        let read = self.offset.saturating_sub(held * BLOCK).max(BLOCK);
        self.inner.hide(kept.min(read));
        self.offset += BLOCK;
    }

    /// One commit that syncs one entry with no samples of each index at the seq of
    /// its durable tail, as a commit that trims and keeps up, then carries the
    /// durable end of each index in the next record.
    pub fn mark(&mut self) {
        self.keep_up();
        for (slot, header) in &self.headers {
            let mut empty = Header {
                len: 0,
                last: Last::Count(0),
                ..*header
            };
            self.inner
                .append(*slot, &mut empty)
                .expect("entries in order");
            self.inner
                .sync(*slot, &empty, self.offset)
                .expect("entries in order");
        }
        self.offset += BLOCK;
        self.carried.clear();
        self.inner
            .idle(self.offset, self.headers.len(), &mut self.carried);
        let tails = self.carried.tails();
        assert_eq!(tails.len(), self.headers.len(), "a tail of each index");
        for (tail, (_, header)) in tails.iter().zip(&self.headers) {
            assert_eq!(tail.index, header.index, "a tail of each index, in order");
        }
        self.inner.hold(&self.carried, self.offset);
        self.offset += BLOCK;
    }

    /// Hides the records before the oldest holder, as a trim that keeps up.
    fn keep_up(&mut self) {
        if let Some(kept) = self.inner.kept() {
            self.inner.hide(kept);
        }
    }
}

/// The header table of one record, as a read or an open finds it.
#[derive(Debug)]
pub struct Table {
    bytes: Vec<u8>,
}

impl Table {
    /// A table of `headers` headers, each of its own index. The first header, and each
    /// `empty`-th header after it, has no samples; none has when `empty` is 0.
    #[must_use]
    pub fn new(headers: u32, empty: u32) -> Self {
        let headers: Vec<Header> = (0..headers)
            .map(|n| {
                let none = empty != 0 && n.is_multiple_of(empty);
                Header {
                    index: channel::Key::from_u128(u128::from(n) + 1),
                    path: Path::Live,
                    first: u64::from(n),
                    len: u32::from(!none),
                    stored_at: Stamp::from_nanos(7),
                    last: if none {
                        Last::Count(1)
                    } else {
                        Last::Stamp(Some(Stamp::from_nanos(9)))
                    },
                    tag: 0,
                    bytes: 0,
                }
            })
            .collect();
        let mut bytes = vec![0; entry::table_len(headers.len())];
        entry::write_table(&headers, &mut bytes);
        Self { bytes }
    }

    /// Decodes each header, as the scan of a read and the walk of an open do, and
    /// gives how many.
    ///
    /// # Panics
    ///
    /// When `new` was given over 1023 headers, the most that a table holds.
    #[must_use]
    pub fn decode(&self) -> usize {
        let headers = entry::parse(&self.bytes, self.bytes.len()).expect("a table");
        let mut decoded = 0;
        for header in headers {
            hint::black_box(header.expect("a header"));
            decoded += 1;
        }
        decoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The restart record and records of 1, 1, 2, 1, 3, 1, and 1 blocks fill 11 of
    /// 15 blocks, and leave the three blocks that stay free.
    #[test]
    fn a_ring_of_15_blocks_takes_a_commit_of_7_records() {
        Ring::new(15).commit(7);
    }

    /// The 8th record, of 2 blocks, does not fit with those three blocks.
    #[test]
    #[should_panic(expected = "the ring has room")]
    fn a_ring_of_15_blocks_refuses_a_commit_of_8_records() {
        Ring::new(15).commit(8);
    }

    /// After the warm-up of the `wal` bench, each commit of 1 record trims on the
    /// rings of the bench, over more than 10 laps of each, so each time holds a trim.
    /// It reads the private writer: `commit` gives nothing back.
    #[test]
    fn each_commit_of_1_record_trims_on_the_rings_of_the_bench() {
        for id in [64, 4096] {
            let mut ring = Ring::new(id + PHASE);
            for _ in 0..2 * id {
                ring.commit(1);
            }
            for commit in 0..11 * id {
                let tail = ring.writer.tail();
                ring.commit(1);
                assert!(
                    ring.writer.tail() > tail,
                    "commit {commit} on the ring of ID {id} trims"
                );
            }
        }
    }

    /// After the warm-up of the `wal` bench, each group commit of 1 and of 32 entries
    /// trims on the ring of the bench, over more than 10 laps, so each time holds a
    /// trim. It reads the private writer: `commit` gives nothing back.
    #[test]
    fn each_group_commit_trims_on_the_ring_of_the_bench() {
        for entries in [1, 32] {
            let mut ring = Group::new(64 + PHASE);
            for _ in 0..2 * 64 {
                ring.commit(entries);
            }
            for commit in 0..11 * 64 {
                let tail = ring.writer.tail();
                ring.commit(entries);
                assert!(
                    ring.writer.tail() > tail,
                    "commit {commit} of {entries} entries trims"
                );
            }
        }
    }

    /// The ring has room for each commit only if each commit releases the space of
    /// the one before.
    #[test]
    fn a_ring_of_64_blocks_commits_past_many_wraps() {
        let mut ring = Ring::new(64);
        for _ in 0..1000 {
            ring.commit(8);
        }
    }

    /// Each commit moves the durable tail of each index past one more entry, and
    /// each log keeps two runs: the run of its holder, and the new one. It reads the
    /// private logs: `commit` gives nothing back, and only the `logs` bench and the
    /// `alloc` and `memory` tests call it, whose counts do not change when a commit
    /// skips the hide.
    #[test]
    fn each_commit_of_the_logs_syncs_one_entry_of_each_index() {
        let mut logs = Logs::new(2);
        for _ in 0..3 {
            logs.commit(2);
        }
        for n in 0..2 {
            let slot = Slot::new(n);
            assert_eq!(logs.inner.durable(slot, Path::Live).seq, 6, "index {n}");
            assert_eq!(logs.inner.runs(slot, Path::Live).count(), 2, "index {n}");
        }
    }

    /// Each carry record becomes the holder of each index, and its tail is the end.
    #[test]
    fn each_carry_of_the_logs_holds_the_tail_of_each_index() {
        let mut logs = Logs::new(2);
        logs.carry();
        logs.carry();
        for n in 0..2 {
            let slot = Slot::new(n);
            assert_eq!(logs.inner.durable(slot, Path::Live).seq, 2, "index {n}");
        }
        assert_eq!(logs.inner.kept(), Some(4 * BLOCK));
    }

    /// Each call moves the appended tail of each index past one more entry.
    #[test]
    fn each_append_of_the_logs_moves_each_index() {
        let mut logs = Logs::new(2);
        logs.append();
        logs.append();
        for n in 0..2 {
            let slot = Slot::new(n);
            assert_eq!(logs.inner.appended(slot, Path::Live).seq, 2, "index {n}");
            assert_eq!(logs.inner.durable(slot, Path::Live).seq, 0, "index {n}");
        }
    }

    /// The last index syncs each third commit, so a trim keeps the records from its
    /// holder on. Its tail is idle once for each holder, and that carry also takes
    /// the tail of the other index.
    #[test]
    fn a_trim_keeps_the_records_after_the_holder_of_the_slow_index() {
        let mut logs = Logs::new(2);
        let mut idle = Vec::new();
        for _ in 0..7 {
            logs.trim(3, 0);
            idle.push(logs.carried.tails().len());
        }
        assert_eq!(idle, [0, 2, 0, 0, 2, 0, 0]);
        let first: Vec<_> = logs.inner.runs(Slot::new(1), Path::Live).collect();
        let second = logs.inner.runs(Slot::new(0), Path::Live).count();
        assert_eq!(
            first.iter().map(|run| run.offset).collect::<Vec<_>>(),
            [7 * BLOCK]
        );
        assert_eq!(second, 1);
        assert_eq!(logs.inner.durable(Slot::new(1), Path::Live).seq, 3);
    }

    /// Each header of a table decodes, with and without headers with no samples.
    #[test]
    fn each_header_of_a_table_decodes() {
        assert_eq!(Table::new(5, 0).decode(), 5);
        assert_eq!(Table::new(5, 2).decode(), 5);
        let table = Table::new(3, 2);
        let headers = entry::parse(&table.bytes, table.bytes.len()).expect("a table");
        let lasts: Vec<Last> = headers
            .map(|header| header.expect("a header").0.last)
            .collect();
        let stamp = Last::Stamp(Some(Stamp::from_nanos(9)));
        assert_eq!(lasts, [Last::Count(1), stamp, Last::Count(1)]);
    }

    /// A read holds the record two commits back, so the runs of the records after it
    /// stay.
    #[test]
    fn a_trim_keeps_the_records_from_the_one_a_read_holds() {
        let mut logs = Logs::new(2);
        for _ in 0..7 {
            logs.trim(3, 2);
        }
        let runs = logs.inner.runs(Slot::new(0), Path::Live);
        let offsets: Vec<u64> = runs.map(|run| run.offset).collect();
        assert_eq!(offsets, [5 * BLOCK, 6 * BLOCK, 7 * BLOCK]);
    }

    /// Each call hides the records before the carry record of the call before, so
    /// only the newest data record keeps a run. An entry with no samples moves no
    /// stamp. It reads the private logs: `mark` gives nothing back, and only the
    /// `logs` bench and the `alloc` test call it. So only this test kills the
    /// mutant that deletes `last: None` in `mark`: it changes only the stamp of a
    /// bench entry, which no caller reads.
    #[test]
    fn each_mark_of_the_logs_hides_the_records_before_the_carry_before() {
        let mut logs = Logs::new(2);
        for _ in 0..3 {
            logs.mark();
        }
        assert_eq!(logs.inner.kept(), Some(6 * BLOCK));
        for n in 0..2 {
            let runs = logs.inner.runs(Slot::new(n), Path::Live);
            let offsets: Vec<u64> = runs.map(|run| run.offset).collect();
            assert_eq!(offsets, [5 * BLOCK], "index {n}");
            let stamp = logs.inner.durable(Slot::new(n), Path::Live).stamp;
            assert_eq!(stamp, None, "index {n}");
        }
    }
}
