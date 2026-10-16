//! The open readers of a shard: their keys, their charge, and what a take gives.

use std::fmt;
use std::mem;
use std::ops::Range;
use std::sync::Arc;

use buffer::{Buffer, Mark, Stored};
pub use delivery::{Error, Position, named};
use delivery::{Reader, Readers, Record, Start};
use types::channel::Slot;
use types::frame::key_set::KeySet;
use types::frame::{Frame, Path};
use types::hash;
use types::time::{Span, Stamp};

pub mod disk;

/// Pool bytes that the entries of one read from disk take at most.
const BUDGET: usize = 256 * 1024;

/// What a reader gets from [`Shard::take`](crate::Shard::take).
#[derive(Debug)]
pub enum Next {
    /// The reader's next frame.
    Frame(Frame),
    /// No frame waits, the next frame waits for a grant or a commit, or the reader is
    /// closed.
    Empty,
    /// The reader's next frames are on disk. Run [`disk::Read::read`] with no borrow
    /// of the shard, give the result to [`Shard::fill`](crate::Shard::fill), then
    /// take again.
    Disk(disk::Read),
    /// No case gives it; #2790 removes it.
    Behind,
}

/// An open reader on its shard, in either mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Key {
    /// The slot of the reader's index.
    pub(crate) slot: Slot,
    /// The reader's session on the index.
    pub(crate) session: delivery::Key,
}

/// A named reader did not open: the node has no mesh time yet, and a named open can
/// close a named complete session, whose hold starts at mesh time. Nothing changed.
/// Open it again later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unsynced;

impl fmt::Display for Unsynced {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the node has no mesh time yet: open the named reader again later"
        )
    }
}

impl std::error::Error for Unsynced {}

/// Why a named complete reader did not open. The open opened nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unopened {
    /// As [`Unsynced`].
    Unsynced,
    /// The open adds a record, and the records of the named readers of the index
    /// would pass `limit` bytes, the most that one entry of the buffer holds. Close a
    /// named reader of the index whose hold is zero, wait for a hold to end, or open
    /// the reader unnamed. An open of a reader that holds a record opens over the
    /// limit, records nothing, and writes a warning event: a restart after its old
    /// hold ends resumes it at the live tail, until #2833.
    Full {
        /// The most bytes of one entry.
        limit: usize,
    },
    /// The open adds a record or opens a held one, and the buffer had no block or no
    /// room for the entry that holds the records, so a restart would not resume the
    /// reader. Open it again later.
    Unrecorded,
}

impl fmt::Display for Unopened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsynced => Unsynced.fmt(f),
            Self::Full { limit } => write!(
                f,
                "the records of the named readers of the index would pass {limit} bytes"
            ),
            Self::Unrecorded => write!(
                f,
                "the home had no room to record the position of the named reader: \
                 open it again later"
            ),
        }
    }
}

impl std::error::Error for Unopened {}

/// A reader that opened, and the session of its name that it took over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opened<K> {
    /// The new reader.
    pub key: K,
    /// The first live seq of the index that the session can get: its open position.
    /// A latest session opens at the first seq of the newest frame of the index, or at
    /// the live tail when there is none.
    pub live: u64,
    /// The reader's session that was open before, in either mode. It is closed: a
    /// take gives `Empty`.
    pub replaced: Option<Key>,
}

/// The key of a reader that takes every frame.
pub mod complete {
    pub use delivery::complete::Charge;
    use types::channel::Slot;

    /// An open complete reader on its shard. It converts into a
    /// [`reader::Key`](super::Key).
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct Key {
        /// The slot of the reader's index.
        pub(crate) slot: Slot,
        /// The reader's session on the index.
        pub(crate) session: delivery::complete::Key,
    }

    impl From<Key> for super::Key {
        fn from(key: Key) -> Self {
            Self {
                slot: key.slot,
                session: key.session.into(),
            }
        }
    }
}

/// The readers of each index of a shard, by the index's place in the shard, and the
/// readers to wake. Each call is on the shard's thread.
#[derive(Debug, Default)]
pub(crate) struct Set {
    /// The readers of each carried index, by place.
    entries: Vec<Entry>,
    /// The place of each index whose readers may be
    /// [pending](delivery::Readers::pending), each once.
    listed: Vec<usize>,
    /// The place of each index with named state ([`Entry::named`]), each once: the
    /// indexes that a tick visits.
    ticked: Vec<usize>,
    /// The records that the open of the shard restored for each index, until its carry
    /// with mesh time, or its first [`advance`](Set::advance) after the carry.
    waiting: hash::Map<Slot, Vec<Record>>,
    /// Each reader to wake: one with a frame to take, or a complete reader that missed
    /// a frame with none waiting. A key can repeat.
    keys: Vec<Key>,
    /// The commits the buffer had ended at the last [`Set::woken`].
    commits: u64,
    /// The number of the first reader key of each index that the set shed and does
    /// not carry now: above each key that the index had, so that a key never names two
    /// readers. An index with no number starts at 0.
    first: hash::Map<Slot, u64>,
    /// Where the next read from disk of each open complete reader that read from disk
    /// goes on. A read from `Mark::at(seq)` gives each entry with no samples at `seq`
    /// again, so more of them than one read takes would stop the reader for ever.
    marks: hash::Map<complete::Key, Mark>,
    /// Each complete reader whose named open appended its record, or took over a
    /// session that waits, until the commit that holds the entry. It takes no frame
    /// until then: a restart before the commit would lose the frames that it got and
    /// did not ack.
    recording: hash::Map<complete::Key, Recording>,
}

/// What a named complete open does to the records of its index.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Opening {
    /// Its session is open with its hold, and the records stay.
    Kept,
    /// The records with its new record: a first open, a reopen in its hold, or a
    /// takeover with a new hold.
    Records(Vec<Record>),
    /// It holds a record, and the records with its new one pass the limit by `bytes`,
    /// as on a smaller pool after a restart. It opens and appends nothing.
    Over { bytes: usize },
}

/// The commit that a named complete reader waits for before it takes a frame.
#[derive(Clone, Copy, Debug)]
struct Recording {
    /// The count of [`Buffer::commits`] at which its record is durable.
    until: u64,
    /// Whether a take found it waiting, so that the commit wakes it.
    waited: bool,
}

/// The readers of one carried index.
#[derive(Debug)]
struct Entry {
    slot: Slot,
    readers: Readers,
    /// Whether `Set::listed` holds the index.
    listed: bool,
    /// Whether `Set::ticked` holds the index.
    ticked: bool,
    /// Whether the newest named entry of the index in the buffer holds a record.
    stored: bool,
}

impl Entry {
    /// Whether the index has named state: its records changed, wait for mesh time in
    /// `waiting`, or are in the buffer, where the end of a hold changes them.
    fn named(&self, waiting: &hash::Map<Slot, Vec<Record>>) -> bool {
        self.stored || self.readers.changed() || waiting.contains_key(&self.slot)
    }
}

impl Set {
    /// A set that carries no index, with the records of the newest named entry of
    /// each index in the buffer, by slot, which the first carry of each index
    /// restores.
    pub(crate) fn new(mut records: hash::Map<Slot, Vec<Record>>) -> Self {
        records.retain(|_, records| !records.is_empty());
        Self {
            waiting: records,
            ..Self::default()
        }
    }

    /// Adds the readers of the index at `slot` at `place`, with `live` as the seq of
    /// its next live frame, and the holds of the records that [`new`](Self::new) got
    /// for it, at mesh time `now`. With no mesh time, the records wait for the first
    /// [`advance`](Self::advance).
    ///
    /// # Panics
    ///
    /// If `place` is not the next place.
    pub(crate) fn carry(
        &mut self,
        place: usize,
        slot: Slot,
        live: u64,
        now: Option<Stamp>,
    ) {
        assert_eq!(place, self.entries.len(), "{place} is not the next place");
        self.entries.push(Entry {
            slot,
            readers: Readers::after(live, self.first.remove(&slot).unwrap_or(0)),
            listed: false,
            ticked: false,
            stored: self.waiting.contains_key(&slot),
        });
        if let Some(now) = now {
            self.restore(place, now);
        }
        self.note(place);
        // Room for every index, so that `applied` never grows the list.
        self.listed.reserve(self.entries.len());
    }

    /// Drops the readers of the index at `place`, and moves the readers of the last
    /// place there, as `Vec::swap_remove` moves an item. Returns whether the newest
    /// named entry of the index holds a record.
    ///
    /// # Panics
    ///
    /// If a reader of the index is open.
    #[must_use]
    pub(crate) fn shed(&mut self, place: usize) -> bool {
        let last = self.entries.len() - 1;
        let entry = self.entries.swap_remove(place);
        self.first.insert(entry.slot, entry.readers.end());
        self.waiting.remove(&entry.slot);
        for places in [&mut self.listed, &mut self.ticked] {
            places.retain(|&listed| listed != place);
            if let Some(listed) = places.iter_mut().find(|listed| **listed == last) {
                *listed = place;
            }
        }
        entry.stored
    }

    /// Adds the holds of the records that wait for mesh time on the index at `place`,
    /// as at mesh time `now`.
    fn restore(&mut self, place: usize, now: Stamp) {
        let entry = &mut self.entries[place];
        if let Some(records) = self.waiting.remove(&entry.slot) {
            entry.readers.restore(records, now);
        }
    }

    /// Adds the index at `place` to [`ticked`](Self::ticked) when it has named state.
    fn note(&mut self, place: usize) {
        let entry = &mut self.entries[place];
        if entry.named(&self.waiting) && !mem::replace(&mut entry.ticked, true) {
            self.ticked.push(place);
        }
    }

    /// The place of each index with named state: its records changed, wait for mesh
    /// time, or are in the buffer, where the end of a hold changes them. Each index
    /// that a tick visits.
    pub(crate) fn ticked(&self) -> &[usize] {
        &self.ticked
    }

    /// Drops each index with no named state from [`ticked`](Self::ticked).
    pub(crate) fn prune(&mut self) {
        let (entries, waiting) = (&mut self.entries, &self.waiting);
        self.ticked.retain(|&place| {
            let entry = &mut entries[place];
            entry.ticked = entry.named(waiting);
            entry.ticked
        });
    }

    /// Whether the records of the index at `place` changed or wait for mesh time, or
    /// a named complete reader of it is open, whose acks move its record: the index
    /// needs a tick soon. Takes time linear in the complete readers of the index.
    pub(crate) fn busy(&self, place: usize) -> bool {
        let entry = &self.entries[place];
        entry.readers.changed()
            || self.waiting.contains_key(&entry.slot)
            || entry.readers.moving()
    }

    /// The mesh time at which the next hold of the index at `place` ends, or `None`
    /// when no named reader of it holds. Takes time linear in its held readers.
    pub(crate) fn deadline(&self, place: usize) -> Option<Stamp> {
        self.entries[place].readers.deadline()
    }

    /// Whether the records of the index at `place` changed since the last
    /// [`recorded`](Self::recorded).
    pub(crate) fn changed(&self, place: usize) -> bool {
        self.entries[place].readers.changed()
    }

    /// The record of each named reader of the index at `place`.
    pub(crate) fn records(&self, place: usize) -> Vec<Record> {
        self.entries[place].readers.records().collect()
    }

    /// Marks the records of the index at `place` as in the buffer: an entry that holds
    /// a record when `stored`.
    pub(crate) fn recorded(&mut self, place: usize, stored: bool) {
        let entry = &mut self.entries[place];
        entry.readers.recorded();
        entry.stored = stored;
    }

    /// Opens an unnamed complete reader on the index at `place` at seq `live`, with a
    /// credit of `limit_bytes` that `charge` charges.
    pub(crate) fn open_complete(
        &mut self,
        place: usize,
        live: u64,
        limit_bytes: u64,
        charge: complete::Charge,
    ) -> delivery::complete::Key {
        let start = Start::At(Position {
            live,
            backfill: None,
        });
        let readers = &mut self.entries[place].readers;
        readers
            .open(Reader::Unnamed, start, limit_bytes, charge)
            .key
    }

    /// Restores the records that wait on the index at `place`, and ends each hold that
    /// ended at or before mesh time `now`. Call it before each named open.
    pub(crate) fn advance(&mut self, place: usize, now: Stamp) {
        self.restore(place, now);
        self.entries[place].readers.advance(now);
        self.note(place);
    }

    /// What a named complete open of `reader` with `hold` does to the records of the
    /// index at `place`, at its held position, else at seq `live`.
    ///
    /// # Errors
    ///
    /// [`Unopened::Full`] when the reader holds no record and the records with its
    /// record pass `limit` bytes, the most of one named entry.
    pub(crate) fn opening(
        &self,
        place: usize,
        reader: &named::Key,
        hold: Span,
        live: u64,
        limit: usize,
    ) -> Result<Opening, Unopened> {
        let mut records = self.records(place);
        let open = |live| Record {
            reader: reader.clone(),
            position: Position {
                live,
                backfill: None,
            },
            hold,
            closed: None,
        };
        let added = match records.iter_mut().find(|record| record.reader == *reader) {
            Some(record) if record.closed.is_none() && record.hold == hold => {
                return Ok(Opening::Kept);
            }
            Some(record) => {
                *record = open(record.position.live);
                false
            }
            None => {
                records.push(open(live));
                true
            }
        };
        let len = crate::named::len(&records);
        if len <= limit {
            Ok(Opening::Records(records))
        } else if added {
            Err(Unopened::Full { limit })
        } else {
            Ok(Opening::Over { bytes: len - limit })
        }
    }

    /// Opens `reader`, which is named, on the index at `place` at its held position,
    /// else at seq `live`, as [`open_complete`](Self::open_complete) does. With
    /// `recorded_at`, the count of [`Buffer::commits`] at which the entry that records
    /// it is durable, it takes no frame until then. A takeover of a session that waits
    /// so waits too.
    ///
    /// # Panics
    ///
    /// If the reader's hold is negative.
    pub(crate) fn open_named_complete(
        &mut self,
        place: usize,
        reader: Reader,
        live: u64,
        limit_bytes: u64,
        charge: complete::Charge,
        recorded_at: Option<u64>,
    ) -> delivery::complete::Opened {
        let start = Start::Resume {
            presented: None,
            otherwise: Position {
                live,
                backfill: None,
            },
        };
        let slot = self.entries[place].slot;
        let opened =
            self.entries[place]
                .readers
                .open(reader, start, limit_bytes, charge);
        let inherited = match opened.replaced {
            Some(delivery::Key::Complete(session)) => self
                .recording
                .get(&complete::Key { slot, session })
                .map(|recording| recording.until),
            _ => None,
        };
        self.replaced(place, opened.replaced);
        self.note(place);
        if let Some(until) = recorded_at.max(inherited) {
            let key = complete::Key {
                slot,
                session: opened.key,
            };
            let waited = false;
            self.recording.insert(key, Recording { until, waited });
        }
        opened
    }

    /// Opens an unnamed latest reader on the index at `place`. It is not woken for the
    /// newest frame it can take at once.
    pub(crate) fn open_latest(&mut self, place: usize) -> delivery::latest::Key {
        self.entries[place].readers.open_latest()
    }

    /// Opens a latest session for the named reader `reader` on the index at `place`,
    /// as [`open_latest`](Self::open_latest) does. A complete session that it takes
    /// over closes at mesh time `now`.
    pub(crate) fn open_named_latest(
        &mut self,
        place: usize,
        reader: named::Key,
        now: Stamp,
    ) -> delivery::latest::Opened {
        let readers = &mut self.entries[place].readers;
        let opened = readers.open_named_latest(reader, now);
        self.replaced(place, opened.replaced);
        opened
    }

    /// The first seq of the newest live frame of the index at `place`, which a latest
    /// session gets at its open, or `None` before the first.
    pub(crate) fn newest(&self, place: usize) -> Option<u64> {
        self.entries[place].readers.newest().map(|frame| {
            let (_, range) = frame
                .ranges()
                .next()
                .expect("invariant: an index frame holds its one group");
            range.seq
        })
    }

    /// Forgets the session `replaced` of the index at `place`, which a takeover
    /// closed.
    fn replaced(&mut self, place: usize, replaced: Option<delivery::Key>) {
        match replaced {
            Some(delivery::Key::Latest(latest)) => self.forget(place, latest),
            Some(delivery::Key::Complete(session)) => self.unmark(place, session),
            None => {}
        }
    }

    /// Drops the mark of the closed complete reader `session` of the index at `place`.
    fn unmark(&mut self, place: usize, session: delivery::complete::Key) {
        let key = complete::Key {
            slot: self.entries[place].slot,
            session,
        };
        self.marks.remove(&key);
        self.recording.remove(&key);
    }

    /// Records that the complete reader `session` on the index at `place` has each
    /// sample below `position`, as [`Readers::ack`] does.
    pub(crate) fn ack(
        &mut self,
        place: usize,
        session: delivery::complete::Key,
        position: Position,
    ) -> Result<(), Error> {
        self.entries[place].readers.ack(session, position)
    }

    /// Raises the credit of the complete reader `session` on the index at `place`, as
    /// [`Readers::grant`] does.
    pub(crate) fn grant(
        &mut self,
        place: usize,
        session: delivery::complete::Key,
        limit_bytes: u64,
    ) {
        self.entries[place].readers.grant(session, limit_bytes);
    }

    /// Takes the next frame of the reader `session` on the index at `place`. A read
    /// from disk reads `buffer`.
    ///
    /// # Panics
    ///
    /// If the index never gave `session`.
    pub(crate) fn take(
        &mut self,
        place: usize,
        session: delivery::Key,
        buffer: &Buffer,
    ) -> Next {
        let entry = &mut self.entries[place];
        if !self.recording.is_empty()
            && let delivery::Key::Complete(session) = session
        {
            let key = complete::Key {
                slot: entry.slot,
                session,
            };
            if let Some(recording) = self.recording.get_mut(&key) {
                if buffer.commits() < recording.until {
                    recording.waited = true;
                    return Next::Empty;
                }
                self.recording.remove(&key);
            }
        }
        let from = match entry.readers.take(session) {
            delivery::Next::Frame(frame) => return Next::Frame(frame),
            delivery::Next::Empty => return Next::Empty,
            delivery::Next::Behind => return Next::Behind,
            delivery::Next::Disk { from } => from,
        };
        let delivery::Key::Complete(session) = session else {
            unreachable!("invariant: a latest session never reads from disk");
        };
        let key = complete::Key {
            slot: entry.slot,
            session,
        };
        let mark = self.marks.entry(key).or_insert(Mark::at(from));
        if mark.seq != from {
            *mark = Mark::at(from);
        }
        Next::Disk(disk::Read {
            key,
            from: *mark,
            budget: BUDGET,
            reader: buffer.reader(),
        })
    }

    /// Gives the frames of `entries` to its session on the index at `place`, whose
    /// live frames on disk end at seq `durable`: the frame of key set `set` that
    /// `decode` makes of each entry, or of none. Drops `entries` when its session
    /// closed after the read started.
    ///
    /// # Errors
    ///
    /// The error of `decode` for the first frame of `entries`, which then gives
    /// nothing. An error after a frame stops the fill there.
    pub(crate) fn fill(
        &mut self,
        place: usize,
        entries: disk::Entries,
        durable: u64,
        set: &Arc<KeySet>,
        mut decode: impl FnMut(&Stored) -> Result<Option<Frame>, block::Error>,
    ) -> Result<(), block::Error> {
        let disk::Entries { key, read } = entries;
        let Some(mark) = self.marks.get_mut(&key) else {
            return Ok(());
        };
        *mark = read.next;
        let entry = &mut self.entries[place];
        let mut given = false;
        for stored in &read.entries {
            let frame = match decode(stored) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(error) if !given => return Err(error),
                Err(_) => return Ok(()),
            };
            given = true;
            let seq = stored.first..stored.first + u64::from(stored.len);
            if !entry.readers.fill(key.session, frame, set, seq) {
                return Ok(());
            }
        }
        entry.readers.reached(key.session, read.next.seq, durable);
        if entry.readers.pending() && !mem::replace(&mut entry.listed, true) {
            self.listed.push(place);
        }
        Ok(())
    }

    /// Closes the reader `session` on the index at `place` at mesh time `now`. Its
    /// waiting frames do not go out, and [`woken`](Self::woken) does not name it. A
    /// close of a closed reader changes nothing.
    ///
    /// # Panics
    ///
    /// If the index never gave `session`, or `now` is `None` and `session` is an open
    /// named complete session.
    pub(crate) fn close(
        &mut self,
        place: usize,
        session: delivery::Key,
        now: Option<Stamp>,
    ) {
        self.entries[place].readers.close(session, now);
        match session {
            delivery::Key::Latest(latest) => self.forget(place, latest),
            delivery::Key::Complete(session) => self.unmark(place, session),
        }
    }

    /// Drops the closed latest reader `session` of the index at `place` from the
    /// readers to wake. Only a latest reader waits there between calls of
    /// [`woken`](Self::woken).
    fn forget(&mut self, place: usize, session: delivery::latest::Key) {
        let key = Key {
            slot: self.entries[place].slot,
            session: session.into(),
        };
        self.keys.retain(|&woken| woken != key);
    }

    /// Gives `frame`, of key set `set`, stored in the buffer at `seq`, to the readers
    /// of the index at `place`. A live frame is the newest frame at once, and goes to
    /// complete readers after the commit that holds it. A backfill frame goes to no
    /// reader.
    pub(crate) fn applied(
        &mut self,
        place: usize,
        frame: Frame,
        set: &Arc<KeySet>,
        seq: Range<u64>,
    ) {
        if frame.path() == Path::Backfill {
            return;
        }
        let entry = &mut self.entries[place];
        entry.readers.queue(&frame, set, seq);
        wake(&mut self.keys, entry.slot, entry.readers.put(frame));
        if entry.readers.pending() && !mem::replace(&mut entry.listed, true) {
            self.listed.push(place);
        }
    }

    /// Makes `frame`, a live frame that found no room in the buffer, the newest frame
    /// of the index at `place`. When the pool had no block for it (`None`), the newest
    /// frame stays. Only latest readers get it.
    ///
    /// # Panics
    ///
    /// If `frame` is a backfill frame, which waits for room instead.
    pub(crate) fn lost(&mut self, place: usize, frame: Option<Frame>) {
        let entry = &mut self.entries[place];
        if let Some(frame) = frame {
            assert_eq!(
                frame.path(),
                Path::Live,
                "invariant: only a live frame is lost"
            );
            wake(&mut self.keys, entry.slot, entry.readers.put(frame));
        }
    }

    /// Replaces `keys` with the readers to wake since the last call, each once, in slot
    /// order and with the latest readers of an index first. Complete readers first get
    /// the live frames on disk in `buffer`. Reads no index when no commit ended since
    /// the last call, as only a commit moves `durable`.
    pub(crate) fn woken(&mut self, buffer: &Buffer, keys: &mut Vec<Key>) {
        let commits = buffer.commits();
        if mem::replace(&mut self.commits, commits) != commits {
            let (entries, woken) = (&mut self.entries, &mut self.keys);
            self.listed.retain(|&place| {
                let entry = &mut entries[place];
                let durable = buffer.durable(entry.slot, Path::Live).seq;
                wake(woken, entry.slot, entry.readers.release(durable));
                entry.listed = entry.readers.pending();
                entry.listed
            });
            let woken = &mut self.keys;
            self.recording.retain(|&key, recording| {
                let recorded = recording.until <= commits;
                if recorded && recording.waited {
                    woken.push(key.into());
                }
                !recorded
            });
        }
        keys.clear();
        keys.append(&mut self.keys);
        keys.sort_unstable();
        keys.dedup();
    }

    /// The place of each index whose readers may be pending.
    #[cfg(test)]
    pub(crate) fn listed(&self) -> &[usize] {
        &self.listed
    }
}

/// Adds the `sessions` of the index at `slot` to `keys`.
fn wake<K: Copy + Into<delivery::Key>>(
    keys: &mut Vec<Key>,
    slot: Slot,
    sessions: &[K],
) {
    if sessions.is_empty() {
        return;
    }
    let woken = sessions.iter().map(|&session| Key {
        slot,
        session: session.into(),
    });
    keys.extend(woken);
}

#[cfg(test)]
mod tests {
    use delivery::complete;
    use types::frame::key_set::{Group, Interner};
    use types::frame::{self, Draft, Form};

    use super::*;
    use crate::common::{create_pool, key};

    /// Index frames of one index with no data channels.
    struct Frames {
        pool: block::Pool,
        set: Arc<KeySet>,
    }

    impl Frames {
        fn new() -> Self {
            let index = Group {
                index: key(Slot::new(0)),
                data: &[],
            };
            Self {
                pool: create_pool(4096),
                set: Interner::new().intern(&[index]),
            }
        }

        /// A frame on `path` with the samples `seq`.
        fn frame(&self, path: Path, seq: Range<u64>) -> Frame {
            let count = u32::try_from(seq.end - seq.start).expect("a short test frame");
            let len = usize::try_from(count).expect("a usize holds a u32") * 8;
            let mut draft = Draft::new(&self.pool, &self.set, Form::Raw, &[(0, len)])
                .expect("the pool has room");
            draft.set_count(0, count);
            draft.set_seq(0, seq.start);
            draft.freeze(path)
        }
    }

    /// A set that carries `indexes` indexes, at slots from 0, each with no live frame.
    fn carried(indexes: u32) -> Set {
        let mut set = Set::default();
        for (place, n) in (0..indexes).enumerate() {
            set.carry(place, Slot::new(n), 0, None);
        }
        set
    }

    /// The readers to wake since the last call, with no commit read.
    fn woken(set: &mut Set) -> Vec<Key> {
        mem::take(&mut set.keys)
    }

    /// The complete readers of the index at `place` that get frames once its live
    /// frames before `durable` are on disk.
    fn released(set: &mut Set, place: usize, durable: u64) -> Vec<complete::Key> {
        set.entries[place].readers.release(durable).to_vec()
    }

    /// The range of each frame that the reader `session` of the index at `place` takes
    /// before [`delivery::Next::Empty`].
    ///
    /// # Panics
    ///
    /// If the reader gets another [`delivery::Next`].
    #[track_caller]
    fn taken(
        set: &mut Set,
        place: usize,
        session: impl Into<delivery::Key>,
    ) -> Vec<frame::Range> {
        let session = session.into();
        let mut ranges = Vec::new();
        loop {
            match set.entries[place].readers.take(session) {
                delivery::Next::Frame(frame) => {
                    ranges.push(frame.range(0).expect("the index is present"));
                }
                delivery::Next::Empty => return ranges,
                end => panic!("{end:?} after {ranges:?}"),
            }
        }
    }

    fn reader(place: u32, session: impl Into<delivery::Key>) -> Key {
        Key {
            slot: Slot::new(place),
            session: session.into(),
        }
    }

    fn range(seq: u64, count: u32) -> frame::Range {
        frame::Range { seq, count }
    }

    mod applied {
        use super::*;

        #[test]
        fn wakes_the_latest_readers_of_a_live_frame_at_once() {
            let frames = Frames::new();
            let mut set = carried(2);
            let latest = set.open_latest(1);
            assert_eq!(woken(&mut set), []);
            set.applied(1, frames.frame(Path::Live, 0..2), &frames.set, 0..2);
            assert_eq!(woken(&mut set), [reader(1, latest)]);
            assert_eq!(set.listed(), []);
            assert_eq!(taken(&mut set, 1, latest), [range(0, 2)]);
        }

        #[test]
        fn queues_live_frames_for_complete_readers_and_lists_the_index_once() {
            let frames = Frames::new();
            let mut set = carried(2);
            let complete = set.open_complete(1, 0, u64::MAX, complete::Charge::Whole);
            set.applied(1, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            set.applied(1, frames.frame(Path::Live, 1..3), &frames.set, 1..3);
            assert_eq!(set.listed(), [1]);
            assert_eq!(woken(&mut set), []);
            assert_eq!(released(&mut set, 1, 3), [complete]);
            assert_eq!(taken(&mut set, 1, complete), [range(0, 1), range(1, 2)]);
        }

        #[test]
        fn gives_a_backfill_frame_to_no_reader() {
            let frames = Frames::new();
            let mut set = carried(1);
            let latest = set.open_latest(0);
            let _: complete::Key =
                set.open_complete(0, 0, u64::MAX, complete::Charge::Whole);
            set.applied(0, frames.frame(Path::Backfill, 0..2), &frames.set, 0..2);
            assert_eq!(woken(&mut set), []);
            assert_eq!(set.listed(), []);
            assert_eq!(released(&mut set, 0, 2), []);
            assert_eq!(taken(&mut set, 0, latest), []);
            set.applied(0, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            assert_eq!(woken(&mut set), [reader(0, latest)]);
        }
    }

    mod lost {
        use super::*;

        #[test]
        fn gives_a_live_frame_only_to_the_latest_readers() {
            let frames = Frames::new();
            let mut set = carried(1);
            let latest = set.open_latest(0);
            let complete = set.open_complete(0, 0, u64::MAX, complete::Charge::Whole);
            set.lost(0, Some(frames.frame(Path::Live, 0..2)));
            assert_eq!(woken(&mut set), [reader(0, latest)]);
            assert_eq!(set.listed(), []);
            assert_eq!(taken(&mut set, 0, latest), [range(0, 2)]);
            assert_eq!(taken(&mut set, 0, complete), []);
            set.applied(0, frames.frame(Path::Live, 2..3), &frames.set, 2..3);
            assert_eq!(released(&mut set, 0, 3), [complete]);
            assert_eq!(taken(&mut set, 0, complete), [range(2, 1)]);
        }

        #[test]
        fn gives_a_complete_reader_the_frames_on_each_side_of_the_lost() {
            let frames = Frames::new();
            let mut set = carried(1);
            let complete = set.open_complete(0, 0, u64::MAX, complete::Charge::Whole);
            set.applied(0, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            set.lost(0, Some(frames.frame(Path::Live, 1..3)));
            set.applied(0, frames.frame(Path::Live, 3..4), &frames.set, 3..4);
            assert_eq!(woken(&mut set), []);
            assert_eq!(released(&mut set, 0, 4), [complete]);
            assert_eq!(taken(&mut set, 0, complete), [range(0, 1), range(3, 1)]);
        }

        #[test]
        #[should_panic(expected = "invariant: only a live frame is lost")]
        fn panics_on_a_backfill_frame() {
            let frames = Frames::new();
            let mut set = carried(1);
            set.lost(0, Some(frames.frame(Path::Backfill, 0..2)));
        }
    }

    mod carry {
        use super::*;

        #[test]
        #[should_panic(expected = "2 is not the next place")]
        fn panics_when_the_place_is_not_the_next() {
            let mut set = carried(1);
            set.carry(2, Slot::new(1), 0, None);
        }

        /// The set keeps the number of a shed index only until it carries it again.
        /// No caller sees a kept number: a slot is carried again only after a `shed`,
        /// which writes over it.
        #[test]
        fn drops_the_number_of_a_shed_index_that_it_carries_again() {
            let mut set = carried(2);
            assert!(!set.shed(0));
            assert_eq!(set.first.len(), 1);
            set.carry(1, Slot::new(0), 0, None);
            assert!(set.first.is_empty());
        }
    }

    mod open_latest {
        use super::*;

        #[test]
        fn does_not_wake_for_the_newest_frame_it_can_take_at_once() {
            let frames = Frames::new();
            let mut set = carried(1);
            set.applied(0, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            let _woken = woken(&mut set);
            let latest = set.open_latest(0);
            assert_eq!(woken(&mut set), []);
            assert_eq!(taken(&mut set, 0, latest), [range(0, 1)]);
        }
    }

    mod close {
        use super::*;

        #[test]
        fn drops_only_the_closed_reader_from_the_readers_to_wake() {
            let frames = Frames::new();
            let mut set = carried(2);
            let first = set.open_latest(0);
            let second = set.open_latest(1);
            assert_eq!(first, second, "each index numbers its own readers");
            set.applied(0, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            set.applied(1, frames.frame(Path::Live, 0..1), &frames.set, 0..1);
            set.close(0, first.into(), None);
            assert_eq!(woken(&mut set), [reader(1, second)]);
        }
    }

    mod disk {
        use super::*;
        use crate::reader::disk::{self as read, Entries};

        /// A set whose complete reader at slot 0 reads from disk from seq 0, after the
        /// live frames `0..durable` were released with no reader open, and the read
        /// of its next entries, `0..durable`, one sample each.
        fn reading(durable: u64) -> (Set, Frames, Entries) {
            let (mut set, frames) = (carried(1), Frames::new());
            for seq in 0..durable {
                set.applied(
                    0,
                    frames.frame(Path::Live, seq..seq + 1),
                    &frames.set,
                    seq..seq + 1,
                );
            }
            released(&mut set, 0, durable);
            let session = set.open_complete(0, 0, u64::MAX, complete::Charge::Whole);
            let key = crate::reader::complete::Key {
                slot: Slot::new(0),
                session,
            };
            let delivery::Next::Disk { from, .. } =
                set.entries[0].readers.take(session.into())
            else {
                panic!("the reader reads from disk");
            };
            set.marks.insert(key, Mark::at(from));
            let bytes = frames.pool.alloc(8).expect("room").freeze();
            let entries = (0..durable)
                .map(|first| Stored {
                    first,
                    len: 1,
                    stored_at: Stamp::from_nanos(0),
                    last: None,
                    tag: 0,
                    bytes: bytes.clone(),
                })
                .collect();
            let read = buffer::Read {
                gap: None,
                entries,
                next: Mark::at(durable),
            };
            (set, frames, Entries { key, read })
        }

        /// Fills `entries` into `set`, with a decode that fails at the entry at
        /// `failed`.
        fn fill(
            set: &mut Set,
            frames: &Frames,
            entries: Entries,
            durable: u64,
            failed: Option<u64>,
        ) -> Result<(), block::Error> {
            set.fill(0, entries, durable, &frames.set, |stored| {
                if Some(stored.first) == failed {
                    return Err(block::Error::Refused { requested: 8 });
                }
                let seq = stored.first..stored.first + 1;
                Ok(Some(frames.frame(Path::Live, seq)))
            })
        }

        #[test]
        fn gives_each_frame_and_goes_live_at_the_durable_end() {
            let (mut set, frames, read) = reading(3);
            let session = read.key.session;
            assert_eq!(fill(&mut set, &frames, read, 3, None), Ok(()));
            assert_eq!(
                taken(&mut set, 0, session),
                [range(0, 1), range(1, 1), range(2, 1)]
            );
            assert_eq!(set.listed(), [], "no live frame waits");
        }

        #[test]
        fn gives_the_error_of_the_first_frame_and_nothing() {
            let (mut set, frames, read) = reading(3);
            let session = read.key.session;
            assert_eq!(
                fill(&mut set, &frames, read, 3, Some(0)),
                Err(block::Error::Refused { requested: 8 })
            );
            assert!(
                matches!(
                    set.entries[0].readers.take(session.into()),
                    delivery::Next::Disk { from: 0, .. }
                ),
                "the reader reads again"
            );
        }

        #[test]
        fn stops_at_an_error_after_a_frame() {
            let (mut set, frames, read) = reading(3);
            let session = read.key.session;
            assert_eq!(fill(&mut set, &frames, read, 3, Some(1)), Ok(()));
            let readers = &mut set.entries[0].readers;
            let first = readers.take(session.into());
            assert!(matches!(first, delivery::Next::Frame(_)), "{first:?}");
            let next = readers.take(session.into());
            assert!(
                matches!(next, delivery::Next::Disk { from: 1, .. }),
                "{next:?}"
            );
        }

        /// No caller sees a kept mark: it shows only as memory.
        #[test]
        fn drops_the_mark_at_a_close_and_a_takeover() {
            let (mut set, _, read) = reading(1);
            set.close(0, read.key.session.into(), None);
            assert_eq!(set.marks.len(), 0, "the close drops the mark");
            let (mut set, _, read) = reading(1);
            set.replaced(0, Some(read.key.session.into()));
            assert_eq!(set.marks.len(), 0, "the takeover drops the mark");
        }

        #[test]
        fn says_why_a_read_failed() {
            let files = env::files::Error::Full {
                path: "ring".into(),
            };
            assert_eq!(
                read::Error::Files(files).to_string(),
                "a read from disk failed: no room on the disk for ring"
            );
            assert_eq!(
                read::Error::Pool(block::Error::Refused { requested: 8 }).to_string(),
                "the pool has no block for a read: the system refused memory for a \
                 block of 8 bytes"
            );
        }
    }

    mod named {
        use super::*;

        fn named(name: &str) -> delivery::named::Key {
            delivery::named::Key {
                subject: "s".parse().expect("a valid name"),
                name: name.parse().expect("a valid name"),
            }
        }

        fn open(set: &mut Set, name: &str) -> complete::Key {
            let reader = Reader::Named {
                reader: named(name),
                hold: Span::from_nanos(10),
            };
            let charge = complete::Charge::Whole;
            set.open_named_complete(0, reader, 0, u64::MAX, charge, None)
                .key
        }

        fn record(name: &str, closed: Option<i64>) -> Record {
            Record {
                reader: named(name),
                position: Position {
                    live: 3,
                    backfill: None,
                },
                hold: Span::from_nanos(10),
                closed: closed.map(Stamp::from_nanos),
            }
        }

        /// The bytes of one named entry that holds a record of each of `names`.
        fn bytes(names: &[&str]) -> usize {
            let records: Vec<_> = names.iter().map(|name| record(name, None)).collect();
            crate::named::len(&records)
        }

        /// A set that restores `records` at the first carry of the index at each slot,
        /// and carries `indexes` indexes, at slots from 0, with no mesh time.
        fn restoring(records: &[(u32, Vec<Record>)], indexes: u32) -> Set {
            let records = records.iter().map(|(n, r)| (Slot::new(*n), r.clone()));
            let mut set = Set::new(records.collect());
            for (place, n) in (0..indexes).enumerate() {
                set.carry(place, Slot::new(n), 0, None);
            }
            set
        }

        fn opened(name: &str, live: u64) -> Record {
            Record {
                position: Position {
                    live,
                    backfill: None,
                },
                ..record(name, None)
            }
        }

        #[test]
        fn change_until_recorded() {
            let mut set = carried(1);
            assert!(!set.changed(0));
            assert!(set.ticked().is_empty());
            let a = open(&mut set, "a");
            assert!(set.changed(0));
            assert_eq!(set.records(0).len(), 1);
            assert_eq!(set.ticked(), [0]);
            set.prune();
            assert_eq!(set.ticked(), [0], "the records changed");
            set.recorded(0, true);
            assert!(!set.changed(0));
            set.prune();
            assert_eq!(set.ticked(), [0], "the buffer holds a record");
            set.close(0, a.into(), Some(Stamp::from_nanos(1)));
            assert!(set.changed(0), "the close changed them");
            assert_eq!(set.records(0)[0].closed, Some(Stamp::from_nanos(1)));
        }

        #[test]
        fn leave_the_tick_once_an_empty_entry_is_recorded() {
            let mut set = carried(1);
            let a = open(&mut set, "a");
            set.recorded(0, true);
            set.close(0, a.into(), Some(Stamp::from_nanos(1)));
            set.recorded(0, true);
            set.advance(0, Stamp::from_nanos(11));
            assert!(set.changed(0));
            assert_eq!(set.records(0), []);
            set.recorded(0, false);
            set.prune();
            assert!(set.ticked().is_empty());
        }

        #[test]
        fn list_only_the_indexes_with_named_state() {
            let mut set = restoring(&[(3, vec![record("a", None)])], 3);
            let _b = open(&mut set, "b");
            set.recorded(0, true);
            let latest = set.open_latest(1);
            set.close(1, latest.into(), None);
            let _c = open(&mut set, "c");
            let mut ticked = set.ticked().to_vec();
            ticked.sort_unstable();
            assert_eq!(ticked, [0]);
            set.carry(3, Slot::new(3), 0, None);
            assert_eq!(set.ticked(), [0, 3]);
        }

        #[test]
        fn keep_the_place_of_each_listed_index_at_a_shed() {
            let records = [
                (3, vec![record("a", Some(1))]),
                (4, vec![record("b", Some(1))]),
            ];
            let set = &mut restoring(&records, 5);
            assert_eq!(set.ticked(), [3, 4]);
            assert!(set.shed(3), "the record is in the buffer");
            assert_eq!(set.ticked(), [3], "the index at 4 moved to 3");
            set.advance(3, Stamp::from_nanos(2));
            assert_eq!(set.records(3), [record("b", Some(1))]);
        }

        #[test]
        fn are_busy_while_records_change_wait_or_move_with_acks() {
            let mut set = restoring(&[(1, vec![record("b", Some(1))])], 1);
            assert!(!set.busy(0));
            let a = open(&mut set, "a");
            assert!(set.busy(0), "the records changed");
            set.recorded(0, true);
            assert!(set.busy(0), "an ack can move the record");
            set.close(0, a.into(), Some(Stamp::from_nanos(1)));
            set.recorded(0, true);
            assert!(!set.busy(0), "only a hold");
            set.carry(1, Slot::new(1), 0, None);
            assert!(set.busy(1), "the records wait");
            assert!(!set.busy(0), "each index is busy alone");
        }

        #[test]
        fn end_at_the_earliest_hold_of_the_index() {
            let mut set = carried(2);
            assert_eq!(set.deadline(0), None);
            let a = open(&mut set, "a");
            let b = open(&mut set, "b");
            set.close(0, a.into(), Some(Stamp::from_nanos(5)));
            set.close(0, b.into(), Some(Stamp::from_nanos(3)));
            assert_eq!(set.deadline(0), Some(Stamp::from_nanos(13)));
            assert_eq!(set.deadline(1), None, "each index ends alone");
            set.advance(0, Stamp::from_nanos(13));
            assert_eq!(set.deadline(0), Some(Stamp::from_nanos(15)));
        }

        #[test]
        fn wait_for_mesh_time_at_a_carry_with_none() {
            let mut set = restoring(&[(0, vec![record("a", None)])], 1);
            assert_eq!(set.ticked(), [0], "the records wait");
            assert!(!set.changed(0));
            set.advance(0, Stamp::from_nanos(4));
            assert!(set.changed(0), "the open session closed at 4");
            assert_eq!(set.records(0), [record("a", Some(4))]);
            set.recorded(0, true);
            set.advance(0, Stamp::from_nanos(5));
            assert!(!set.changed(0), "a second advance restores nothing");
        }

        #[test]
        fn restore_at_a_carry_with_mesh_time() {
            let records = vec![record("a", None), record("b", Some(1))];
            let mut set = Set::new([(Slot::new(0), records)].into_iter().collect());
            set.carry(0, Slot::new(0), 0, Some(Stamp::from_nanos(4)));
            assert!(set.changed(0), "the open session closed at 4");
            let mut restored = set.records(0);
            restored.sort_by(|a, b| a.reader.name.cmp(&b.reader.name));
            assert_eq!(restored, [record("a", Some(4)), record("b", Some(1))]);
        }

        #[test]
        fn restore_no_empty_entry() {
            let set = restoring(&[(0, Vec::new())], 1);
            assert!(set.ticked().is_empty(), "the buffer holds no record");
        }

        #[test]
        fn shed_says_whether_the_buffer_holds_a_record() {
            let mut set = restoring(&[(2, vec![record("a", Some(1))])], 3);
            assert!(!set.shed(0));
            assert!(set.shed(0), "the restored records are in the buffer");
        }

        #[test]
        fn add_the_record_of_a_first_open_up_to_the_limit() {
            let mut set = carried(1);
            let _a = open(&mut set, "a");
            let _b = open(&mut set, "b");
            let limit = bytes(&["a", "b", "c"]);
            let hold = Span::from_nanos(10);
            let Ok(Opening::Records(mut added)) =
                set.opening(0, &named("c"), hold, 3, limit)
            else {
                panic!("it adds a record that fits");
            };
            added.sort_by(|a, b| a.reader.name.cmp(&b.reader.name));
            assert_eq!(added, [opened("a", 0), opened("b", 0), record("c", None)]);
            assert_eq!(
                set.opening(0, &named("cd"), hold, 3, limit),
                Err(Unopened::Full { limit })
            );
        }

        #[test]
        fn add_nothing_at_a_takeover_of_an_open_session_with_its_hold() {
            let mut set = carried(1);
            let _a = open(&mut set, "a");
            let hold = Span::from_nanos(10);
            let kept = set.opening(0, &named("a"), hold, 3, usize::MAX);
            assert_eq!(kept, Ok(Opening::Kept));
        }

        #[test]
        fn change_the_hold_at_a_takeover_of_an_open_session() {
            let mut set = carried(1);
            let _a = open(&mut set, "a");
            let hold = Span::from_nanos(20);
            let records = set.opening(0, &named("a"), hold, 3, usize::MAX);
            let taken = Record {
                hold,
                ..opened("a", 0)
            };
            assert_eq!(records, Ok(Opening::Records(vec![taken])));
        }

        #[test]
        fn open_a_held_record_at_its_position_with_the_new_hold() {
            let mut set = carried(1);
            let a = open(&mut set, "a");
            set.ack(
                0,
                a,
                Position {
                    live: 2,
                    backfill: None,
                },
            )
            .expect("forward");
            set.close(0, a.into(), Some(Stamp::from_nanos(1)));
            let hold = Span::from_nanos(20);
            let records = set.opening(0, &named("a"), hold, 3, usize::MAX);
            let reopened = Record {
                hold,
                ..opened("a", 2)
            };
            assert_eq!(records, Ok(Opening::Records(vec![reopened])));
        }

        #[test]
        fn open_a_held_record_past_the_limit_with_no_append() {
            let mut set = carried(1);
            let a = open(&mut set, "a");
            set.close(0, a.into(), Some(Stamp::from_nanos(1)));
            let hold = Span::from_nanos(10);
            let bytes = crate::named::len(&[Record {
                hold,
                ..opened("a", 0)
            }]);
            let over = set.opening(0, &named("a"), hold, 3, 1);
            assert_eq!(over, Ok(Opening::Over { bytes: bytes - 1 }));
        }

        #[test]
        fn take_over_with_a_new_hold_past_the_limit_with_no_append() {
            let mut set = carried(1);
            open(&mut set, "a");
            let hold = Span::from_nanos(20);
            let bytes = crate::named::len(&[Record {
                hold,
                ..opened("a", 0)
            }]);
            let over = set.opening(0, &named("a"), hold, 3, 1);
            assert_eq!(over, Ok(Opening::Over { bytes: bytes - 1 }));
        }

        #[test]
        fn says_why_a_named_reader_did_not_open() {
            assert_eq!(
                Unsynced.to_string(),
                "the node has no mesh time yet: open the named reader again later"
            );
            assert_eq!(
                Unopened::Unsynced.to_string(),
                "the node has no mesh time yet: open the named reader again later"
            );
            assert_eq!(
                Unopened::Full { limit: 4032 }.to_string(),
                "the records of the named readers of the index would pass 4032 bytes"
            );
        }
    }
}
