//! The named entries of an index. Their bodies hold the record of each named reader of
//! the index, open or held, from the entry's place in the index log, in up to
//! [`PARTS`] parts. Part `p` is the entry of tag `2 + p`, and the newest entry of a
//! part replaces the ones before it.

use std::collections::BTreeMap;
use std::num::NonZeroU8;
use std::str;

use buffer::{Buffer, Layout};
use delivery::{Position, Record};
use types::channel::{self, Slot};
use types::frame::Path;
use types::hash;
use types::name::Name;
use types::time::{Span, Stamp};

use crate::state;

/// The most parts of the named records of one index.
const PARTS: u8 = 64;

/// The buffer tag of part 0.
const TAG: u8 = 2;

/// The version of the body that this build writes and reads.
const VERSION: u8 = 1;

/// The bytes of the body before the records: the version and the count.
const HEAD: usize = 1 + 4;

/// The bytes of a record apart from its subject and name.
const FIXED: usize = 1 + 1 + 8 + 1 + 8 + 8 + 1 + 8;

/// The most bytes of one record: the longest subject and name.
pub(crate) const RECORD_MAX: usize = FIXED + 2 * Name::MAX_BYTES;

/// The most bytes of the body of one part: one entry of any ring, in one block of any
/// pool.
const PART_MAX: usize = Layout::ENTRY_MAX_MIN;
const _: () = assert!(HEAD + 7 * RECORD_MAX <= PART_MAX, "7 records fit one part");

/// The most bytes of the records of the named readers of one index.
pub(crate) const LIMIT: usize = PARTS as usize * (PART_MAX - HEAD);

/// The bytes of the record of `reader` in a body.
fn record_len(reader: &delivery::named::Key) -> usize {
    FIXED + reader.subject.as_str().len() + reader.name.as_str().len()
}

/// The buffer tag of `part`.
fn tag(part: u8) -> NonZeroU8 {
    NonZeroU8::new(TAG + part).expect("invariant: a tag of a part is not 0")
}

/// No part has room for a new record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Full;

/// How far an [`append`](Parts::append) went: it appended each part before `until`
/// whose body changed, and none from `until` on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Appended {
    until: u8,
}

impl Appended {
    /// Whether it appended each part whose body changed.
    pub(crate) fn all(self) -> bool {
        self.until == PARTS
    }
}

/// The named records of one index in the buffer: the part of each record, and the
/// body that each part last appended.
#[derive(Debug, Default)]
pub(crate) struct Parts {
    /// The part of each record, and of each removed one until its removal commits, so
    /// that a key is in one part only.
    keys: BTreeMap<delivery::named::Key, Placed>,
    /// The body that each part last appended, by part. Empty for a part with no entry.
    bodies: Vec<Vec<u8>>,
}

/// The place of the record of one key.
#[derive(Clone, Copy, Debug)]
struct Placed {
    part: u8,
    state: Kept,
}

/// What the parts keep of the record of one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kept {
    /// The records of the last [`Parts::append`] hold it.
    Held,
    /// The records of the last append do not hold it, and no entry of its part has
    /// dropped it yet.
    Removing,
    /// An entry of its part dropped it, durable at this count of [`Buffer::commits`].
    Removed(u64),
}

impl Parts {
    /// The parts that the newest durable named entries of each index in `buffer` hold,
    /// with the records of each, by the index's slot. An index with no named entry is
    /// absent. It reads the tags of the parts from part 0 up, to the first tag that no
    /// index has an entry of.
    ///
    /// # Errors
    ///
    /// The error of [`Buffer::newest`].
    ///
    /// # Panics
    ///
    /// When a named entry does not decode, as [`read`] says, or a key is in two parts.
    pub(crate) async fn newest(
        buffer: &Buffer,
    ) -> Result<hash::Map<Slot, (Self, Vec<Record>)>, buffer::Error> {
        let mut found: hash::Map<Slot, (Self, Vec<Record>)> = hash::Map::default();
        for part in 0..PARTS {
            let newest = buffer.newest(Path::Live, tag(part)).await?;
            if newest.is_empty() {
                break;
            }
            for (slot, stored) in newest {
                let (parts, records) = found.entry(slot).or_default();
                let body = stored.bytes.to_vec();
                for record in read(&body) {
                    let placed = Placed {
                        part,
                        state: Kept::Held,
                    };
                    if let Some(other) =
                        parts.keys.insert(record.reader.clone(), placed)
                    {
                        panic!(
                            "the named reader {:?} is in parts {} and {part}",
                            record.reader, other.part
                        );
                    }
                    records.push(record);
                }
                parts.keep(part, body);
            }
        }
        Ok(found)
    }

    /// Appends each part whose body differs from the body it last appended, so that
    /// the parts hold `records` and no other record, on the live path of the index
    /// `index` at `slot`, at seq `first` and mesh time `stored_at`. Each part goes
    /// alone, in part order, and the append stops at the first part that finds no
    /// block or no room. A record keeps its part. A new one goes in the lowest part
    /// with room for it. A removed record keeps its room until the commit of the entry
    /// that drops it, and goes back in its part when it returns before that.
    ///
    /// # Errors
    ///
    /// [`Full`] when no part has room for a new record of `records`. It then appends
    /// nothing.
    ///
    /// # Panics
    ///
    /// If `records` hold a key twice.
    pub(crate) fn append(
        &mut self,
        buffer: &Buffer,
        records: &[Record],
        index: channel::Key,
        slot: Slot,
        first: u64,
        stored_at: Stamp,
    ) -> Result<Appended, Full> {
        let added = self.place(records, buffer.commits())?;
        for (part, body) in self.changed(records) {
            let appended = state::append(buffer, |pool| {
                let mut block = pool.alloc(body.len())?;
                block.copy_from_slice(&body);
                let body = Some(block.freeze());
                Ok(state::entry(tag(part), body, index, slot, first, stored_at))
            });
            if !appended {
                for key in added {
                    if self.keys[key].part >= part {
                        self.keys.remove(key);
                    }
                }
                return Ok(Appended { until: part });
            }
            self.appended(part, body, buffer.durable_at());
        }
        Ok(Appended { until: PARTS })
    }

    /// Places each record of `records` in its part, after it drops each removed
    /// record whose removal is durable by the count of [`Buffer::commits`] `commits`,
    /// and returns the keys that it added.
    ///
    /// # Errors
    ///
    /// [`Full`] when no part has room for a new record. It then adds no key.
    ///
    /// # Panics
    ///
    /// If `records` hold a key twice.
    fn place<'r>(
        &mut self,
        records: &'r [Record],
        commits: u64,
    ) -> Result<Vec<&'r delivery::named::Key>, Full> {
        for placed in self.keys.values_mut() {
            if placed.state == Kept::Held {
                placed.state = Kept::Removing;
            }
        }
        let mut added = Vec::new();
        for record in records {
            match self.keys.get_mut(&record.reader) {
                Some(placed) => placed.state = Kept::Held,
                None => added.push(&record.reader),
            }
        }
        self.release(commits);
        let mut used = vec![HEAD; usize::from(PARTS)];
        for (key, placed) in &self.keys {
            used[usize::from(placed.part)] += record_len(key);
        }
        for (at, &key) in added.iter().enumerate() {
            let len = record_len(key);
            let room = |part: &u8| used[usize::from(*part)] + len <= PART_MAX;
            let Some(part) = (0..PARTS).find(room) else {
                for key in &added[..at] {
                    self.keys.remove(*key);
                }
                return Err(Full);
            };
            used[usize::from(part)] += len;
            let state = Kept::Held;
            let old = self.keys.insert(key.clone(), Placed { part, state });
            assert!(old.is_none(), "the records hold {key:?} twice");
        }
        Ok(added)
    }

    /// The body of each part, in part order, that differs from the body it last
    /// appended, when the parts hold `records`, which [`place`](Self::place) placed.
    fn changed(&self, records: &[Record]) -> Vec<(u8, Vec<u8>)> {
        let mut sorted: Vec<(u8, &Record)> = records
            .iter()
            .map(|record| (self.keys[&record.reader].part, record))
            .collect();
        sorted.sort_unstable_by(|a, b| (a.0, &a.1.reader).cmp(&(b.0, &b.1.reader)));
        let used = sorted.last().map_or(0, |&(part, _)| usize::from(part) + 1);
        let mut rest = &sorted[..];
        let mut changed = Vec::new();
        for part in 0..used.max(self.bodies.len()) {
            let (now, after) =
                rest.split_at(rest.partition_point(|&(at, _)| usize::from(at) == part));
            rest = after;
            let last = self.bodies.get(part).map_or(&[][..], Vec::as_slice);
            if now.is_empty() && last.is_empty() {
                continue;
            }
            let mut body = vec![0; len(now.iter().map(|&(_, record)| record))];
            write(&mut body, now.iter().map(|&(_, record)| record));
            if body != last {
                let part = u8::try_from(part).expect("invariant: a part fits a u8");
                changed.push((part, body));
            }
        }
        changed
    }

    /// Keeps `body` as the body that `part` last appended, in an entry durable at the
    /// count of [`Buffer::commits`] `durable_at`.
    fn appended(&mut self, part: u8, body: Vec<u8>, durable_at: u64) {
        for placed in self.keys.values_mut() {
            if placed.part == part && placed.state == Kept::Removing {
                placed.state = Kept::Removed(durable_at);
            }
        }
        self.keep(part, body);
    }

    /// Whether the record of `reader` is in a part before the first that `appended`
    /// did not append.
    pub(crate) fn recorded(
        &self,
        reader: &delivery::named::Key,
        appended: Appended,
    ) -> bool {
        let placed = self.keys.get(reader);
        placed.is_some_and(|placed| placed.part < appended.until)
    }

    /// Drops each removed record whose removal is durable by the count of
    /// [`Buffer::commits`] `commits`. Returns whether a part may still hold a record.
    pub(crate) fn release(&mut self, commits: u64) -> bool {
        self.keys.retain(|_, placed| match placed.state {
            Kept::Removed(at) => at > commits,
            Kept::Held | Kept::Removing => true,
        });
        !self.keys.is_empty()
    }

    /// Keeps `body` as the body that `part` last appended.
    fn keep(&mut self, part: u8, body: Vec<u8>) {
        let part = usize::from(part);
        if self.bodies.len() <= part {
            self.bodies.resize_with(part + 1, Vec::new);
        }
        self.bodies[part] = body;
    }
}

/// The bytes of the body that holds `records`.
fn len<'a>(records: impl IntoIterator<Item = &'a Record>) -> usize {
    let records = records.into_iter().map(|record| record_len(&record.reader));
    records.sum::<usize>() + HEAD
}

/// Writes `records` to `out`, which has [`len`] bytes.
fn write<'a>(out: &mut [u8], records: impl ExactSizeIterator<Item = &'a Record>) {
    let count = u32::try_from(records.len()).expect("at most u32::MAX records");
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        out[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&[VERSION]);
    put(&count.to_le_bytes());
    for record in records {
        for name in [&record.reader.subject, &record.reader.name] {
            let name = name.as_str().as_bytes();
            put(&[u8::try_from(name.len()).expect("a name fits 255 bytes")]);
            put(name);
        }
        put(&record.position.live.to_le_bytes());
        put(&[u8::from(record.position.backfill.is_some())]);
        put(&record.position.backfill.unwrap_or(0).to_le_bytes());
        put(&record.hold.nanos().to_le_bytes());
        put(&[u8::from(record.closed.is_some())]);
        put(&record.closed.map_or(0, Stamp::nanos).to_le_bytes());
    }
}

/// The records that `body`, the bytes of a named entry, holds.
///
/// # Panics
///
/// If the version is not 1, or the bytes are not a body of that version. Bytes from
/// another node must be checked before they reach `read`.
fn read(body: &[u8]) -> Vec<Record> {
    let mut rest = body;
    let version = take::<1>(&mut rest)[0];
    assert!(
        version == VERSION,
        "the named entry has version {version}; this build reads version {VERSION}"
    );
    let count = u32::from_le_bytes(take(&mut rest));
    let count = usize::try_from(count).expect("invariant: a usize holds a u32");
    // A count from disk sizes the vector only when the bytes can hold it.
    assert!(
        count <= rest.len() / FIXED,
        "the named entry ends inside a record"
    );
    let records = (0..count)
        .map(|_| {
            let subject = name(&mut rest);
            let name = name(&mut rest);
            let live = u64::from_le_bytes(take(&mut rest));
            let backfill = flagged(&mut rest).map(u64::from_le_bytes);
            let hold = Span::from_nanos(i64::from_le_bytes(take(&mut rest)));
            let closed = flagged(&mut rest)
                .map(|bytes| Stamp::from_nanos(i64::from_le_bytes(bytes)));
            Record {
                reader: delivery::named::Key { subject, name },
                position: Position { live, backfill },
                hold,
                closed,
            }
        })
        .collect();
    assert!(
        rest.is_empty(),
        "the named entry has {} bytes past its records",
        rest.len()
    );
    records
}

/// The next `N` bytes of `rest`.
///
/// # Panics
///
/// If `rest` holds fewer than `N` bytes.
fn take<const N: usize>(rest: &mut &[u8]) -> [u8; N] {
    let Some((bytes, after)) = rest.split_first_chunk() else {
        panic!("the named entry ends inside a record");
    };
    *rest = after;
    *bytes
}

/// The next value of `rest` after its flag: `None` for a flag of 0.
///
/// # Panics
///
/// If the flag is not 0 or 1, or `rest` ends first.
fn flagged(rest: &mut &[u8]) -> Option<[u8; 8]> {
    let flag = take::<1>(rest)[0];
    let value = take(rest);
    match flag {
        0 => None,
        1 => Some(value),
        flag => panic!("the named entry has the flag {flag}"),
    }
}

/// The next name of `rest`, after its length.
///
/// # Panics
///
/// If `rest` ends first, or the bytes are not a name.
fn name(rest: &mut &[u8]) -> Name {
    let len = usize::from(take::<1>(rest)[0]);
    let Some((bytes, after)) = rest.split_at_checked(len) else {
        panic!("the named entry ends inside a record");
    };
    *rest = after;
    str::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("a named entry name is not UTF-8: {error}"))
        .parse()
        .unwrap_or_else(|error| panic!("a named entry name is not a name: {error}"))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn record(
        subject: &str,
        name: &str,
        backfill: Option<u64>,
        closed: Option<i64>,
    ) -> Record {
        Record {
            reader: delivery::named::Key {
                subject: subject.parse().expect("a valid name"),
                name: name.parse().expect("a valid name"),
            },
            position: Position {
                live: 0x0102,
                backfill,
            },
            hold: Span::from_nanos(0x0304),
            closed: closed.map(Stamp::from_nanos),
        }
    }

    fn encoded(records: &[Record]) -> Vec<u8> {
        let mut body = vec![0; len(records)];
        write(&mut body, records.iter());
        body
    }

    #[test]
    fn gives_part_p_the_tag_2_plus_p() {
        assert_eq!((tag(0).get(), tag(PARTS - 1).get()), (2, 65));
    }

    mod write {
        use super::*;

        #[test]
        fn writes_the_version_the_count_then_each_record() {
            let bytes = encoded(&[
                record("ops", "r", Some(5), Some(7)),
                record("a", "b", None, None),
            ]);

            let mut expected = vec![1, 2, 0, 0, 0];
            expected.extend(b"\x03ops\x01r");
            expected.extend([2, 1, 0, 0, 0, 0, 0, 0]);
            expected.extend([1, 5, 0, 0, 0, 0, 0, 0, 0]);
            expected.extend([4, 3, 0, 0, 0, 0, 0, 0]);
            expected.extend([1, 7, 0, 0, 0, 0, 0, 0, 0]);
            expected.extend(b"\x01a\x01b");
            expected.extend([2, 1, 0, 0, 0, 0, 0, 0]);
            expected.extend([0; 9]);
            expected.extend([4, 3, 0, 0, 0, 0, 0, 0]);
            expected.extend([0; 9]);
            assert_eq!(bytes, expected);
        }

        #[test]
        fn writes_the_largest_record_for_the_longest_names() {
            let longest = "b".repeat(Name::MAX_BYTES);
            let records = [record(&longest, &longest, Some(1), Some(1))];

            assert_eq!(encoded(&records).len(), HEAD + RECORD_MAX);
            assert_eq!((RECORD_MAX, len(&records), LIMIT), (546, 551, 257_728));
        }
    }

    /// A record of the longest subject and name, with a subject that starts with `n`.
    fn longest(n: usize) -> Record {
        let n = n.to_string();
        let subject = format!("{n}{}", "s".repeat(Name::MAX_BYTES - n.len()));
        record(&subject, &"r".repeat(Name::MAX_BYTES), None, None)
    }

    fn part(parts: &Parts, record: &Record) -> u8 {
        parts.keys[&record.reader].part
    }

    /// The bytes of the records that each part keeps room for, with the head.
    fn used(parts: &Parts) -> Vec<usize> {
        let mut used = vec![HEAD; usize::from(PARTS)];
        for (key, placed) in &parts.keys {
            used[usize::from(placed.part)] += record_len(key);
        }
        used
    }

    /// Places `records`, and appends each part that changes, durable at `at`.
    fn append(parts: &mut Parts, records: &[Record], commits: u64, at: u64) {
        parts.place(records, commits).expect("room");
        for (part, body) in parts.changed(records) {
            parts.appended(part, body, at);
        }
    }

    mod parts {
        use super::*;

        #[test]
        fn places_seven_of_the_longest_records_in_each_part() {
            let mut records: Vec<_> = (0..448).map(longest).collect();
            let mut parts = Parts::default();

            assert_eq!(parts.place(&records, 0).expect("room").len(), 448);

            for (n, record) in records.iter().enumerate() {
                assert_eq!(usize::from(part(&parts, record)), n / 7, "record {n}");
            }
            records.push(longest(448));
            assert_eq!(parts.place(&records, 0), Err(Full));
            assert!(!parts.keys.contains_key(&longest(448).reader));
        }

        #[test]
        fn adds_no_key_when_a_later_record_has_no_room() {
            let mut records: Vec<_> = (0..448).map(longest).collect();
            let mut parts = Parts::default();
            parts.place(&records, 0).expect("room");
            let short = record("a", "b", None, None);
            records.extend([short.clone(), longest(448)]);

            assert_eq!(parts.place(&records, 0), Err(Full));

            assert!(!parts.keys.contains_key(&short.reader));
            assert_eq!(parts.keys.len(), 448);
        }

        #[test]
        #[should_panic(expected = "the records hold")]
        fn panics_on_records_that_hold_a_key_twice() {
            let records = [longest(0), longest(0)];
            Parts::default().place(&records, 0).expect("room");
        }

        #[test]
        fn keeps_a_removed_record_in_its_part_until_its_removal_commits() {
            let records: Vec<_> = (0..7).map(longest).collect();
            let mut parts = Parts::default();
            append(&mut parts, &records, 0, 1);
            append(&mut parts, &records[1..], 1, 5);
            let mut added = records[1..].to_vec();
            added.push(longest(7));

            parts.place(&added, 4).expect("room");

            assert_eq!(part(&parts, &longest(7)), 1, "the removal of 0 waits");
            added.push(longest(0));
            parts.place(&added, 4).expect("room");
            assert_eq!(part(&parts, &longest(0)), 0, "it goes back in its part");
        }

        #[test]
        fn frees_the_room_of_a_removed_record_when_its_removal_commits() {
            let records: Vec<_> = (0..7).map(longest).collect();
            let mut parts = Parts::default();
            append(&mut parts, &records, 0, 1);
            append(&mut parts, &records[1..], 1, 5);
            let mut added = records[1..].to_vec();
            added.push(longest(7));

            parts.place(&added, 5).expect("room");

            assert_eq!(part(&parts, &longest(7)), 0);
            assert!(!parts.keys.contains_key(&longest(0).reader));
        }

        #[test]
        fn keeps_the_room_of_a_record_whose_removal_found_no_append() {
            let records: Vec<_> = (0..7).map(longest).collect();
            let mut parts = Parts::default();
            append(&mut parts, &records, 0, 1);
            parts.place(&records[1..], 9).expect("room");
            let mut added = records[1..].to_vec();
            added.push(longest(7));

            parts.place(&added, 9).expect("room");

            assert_eq!(part(&parts, &longest(7)), 1);
        }

        #[test]
        fn changes_only_the_parts_whose_records_change() {
            let mut records: Vec<_> = (0..14).map(longest).collect();
            let mut parts = Parts::default();
            parts.place(&records, 0).expect("room");
            let changed: Vec<_> = parts.changed(&records).into_iter().collect();
            let parts_of = |changed: &[(u8, Vec<u8>)]| {
                changed.iter().map(|(part, _)| *part).collect::<Vec<_>>()
            };
            assert_eq!(parts_of(&changed), [0, 1]);
            for (part, body) in changed {
                parts.appended(part, body, 1);
            }
            records[8].position.live = 9;

            parts.place(&records, 0).expect("room");
            let changed = parts.changed(&records);

            let mut expected = records[7..].to_vec();
            expected.sort_by(|a, b| a.reader.cmp(&b.reader));
            let read: Vec<_> = changed
                .iter()
                .map(|(part, body)| (*part, read(body)))
                .collect();
            assert_eq!(read, [(1, expected)]);
        }

        #[test]
        fn empties_a_part_whose_records_all_go() {
            let records: Vec<_> = (0..8).map(longest).collect();
            let mut parts = Parts::default();
            append(&mut parts, &records, 0, 1);
            parts.place(&records[..7], 1).expect("room");

            assert_eq!(parts.changed(&records[..7]), [(1, encoded(&[]))]);

            append(&mut parts, &records[..7], 1, 3);
            parts.place(&records[..7], 3).expect("room");
            assert_eq!(parts.changed(&records[..7]), []);
            assert_eq!(parts.keys.len(), 7, "the removal of 7 committed");
        }

        #[test]
        fn appends_no_part_that_never_held_a_record() {
            let mut parts = Parts::default();
            parts.place(&[], 0).expect("room");
            assert_eq!(parts.changed(&[]), []);
        }

        #[test]
        fn records_a_reader_in_a_part_before_the_first_unappended() {
            let records: Vec<_> = (0..8).map(longest).collect();
            let mut parts = Parts::default();
            parts.place(&records, 0).expect("room");
            let appended = Appended { until: 1 };

            assert!(parts.recorded(&records[6].reader, appended));
            assert!(!parts.recorded(&records[7].reader, appended));
            assert!(!parts.recorded(&longest(8).reader, appended));
            assert!(!appended.all());
            assert!(Appended { until: PARTS }.all());
        }

        #[test]
        fn says_whether_a_part_may_hold_a_record_at_a_release() {
            let records = [longest(0)];
            let mut parts = Parts::default();
            append(&mut parts, &records, 0, 1);
            append(&mut parts, &[], 1, 2);

            assert!(parts.release(1), "the removal waits");
            assert!(!parts.release(2));
        }
    }

    mod read {
        use super::*;

        #[test]
        fn reads_no_record_from_an_empty_body() {
            assert_eq!(read(&encoded(&[])), []);
        }

        mod when_damaged {
            use super::*;

            #[test]
            #[should_panic(
                expected = "the named entry has version 2; this build reads version 1"
            )]
            fn panics_on_another_version() {
                read(&[2, 0, 0, 0, 0]);
            }

            #[test]
            #[should_panic(expected = "the named entry ends inside a record")]
            fn panics_on_a_body_that_ends_inside_a_record() {
                let bytes = encoded(&[record("ops", "r", None, None)]);
                read(&bytes[..bytes.len() - 1]);
            }

            #[test]
            #[should_panic(expected = "the named entry ends inside a record")]
            fn panics_on_a_name_past_the_end() {
                let mut bytes = vec![1, 1, 0, 0, 0, 100];
                bytes.extend([b'o'; 40]);
                read(&bytes);
            }

            #[test]
            #[should_panic(expected = "the named entry ends inside a record")]
            fn panics_on_a_count_past_the_end_with_no_allocation() {
                read(&[1, 0xff, 0xff, 0xff, 0xff]);
            }

            #[test]
            #[should_panic(expected = "the named entry has 1 bytes past its records")]
            fn panics_on_bytes_past_the_records() {
                read(&[1, 0, 0, 0, 0, 9]);
            }

            #[test]
            #[should_panic(expected = "the named entry has the flag 2")]
            fn panics_on_a_flag_that_is_not_0_or_1() {
                let mut bytes = encoded(&[record("ops", "r", None, None)]);
                bytes[HEAD + 6 + 8] = 2;
                read(&bytes);
            }

            #[test]
            #[should_panic(
                expected = "a named entry name is not UTF-8: invalid utf-8 \
                                       sequence of 1 bytes from index 0"
            )]
            fn panics_on_a_name_that_is_not_utf8() {
                let mut bytes = vec![1, 1, 0, 0, 0, 1, 0xff];
                bytes.extend([0; 40]);
                read(&bytes);
            }

            #[test]
            #[should_panic(expected = "a named entry name is not a name: a name or \
                                       pattern is empty")]
            fn panics_on_an_empty_name() {
                let mut bytes = vec![1, 1, 0, 0, 0, 0];
                bytes.extend([0; 40]);
                read(&bytes);
            }
        }
    }

    fn any_name() -> impl Strategy<Value = String> {
        "@?[A-Za-z0-9_-]{1,63}(\\.@?[A-Za-z0-9_-]{1,63}){0,3}"
            .prop_filter("a name", |name| name.len() <= Name::MAX_BYTES)
    }

    proptest! {
        #[test]
        fn reads_back_each_body(
            records in prop::collection::vec(
                (
                    any_name(),
                    any_name(),
                    any::<u64>(),
                    any::<Option<u64>>(),
                    any::<i64>(),
                    any::<Option<i64>>(),
                ),
                0..8,
            )
        ) {
            let records: Vec<_> = records
                .into_iter()
                .map(|(subject, name, live, backfill, hold, closed)| Record {
                    reader: delivery::named::Key {
                        subject: subject.parse().expect("a valid name"),
                        name: name.parse().expect("a valid name"),
                    },
                    position: Position { live, backfill },
                    hold: Span::from_nanos(hold),
                    closed: closed.map(Stamp::from_nanos),
                })
                .collect();

            let bytes = encoded(&records);

            prop_assert_eq!(bytes.len(), len(&records));
            prop_assert_eq!(read(&bytes), records);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn places_each_new_record_in_the_lowest_part_with_room(
            lens in prop::collection::vec(
                (4..=Name::MAX_BYTES, 1..=Name::MAX_BYTES),
                1..1200,
            )
        ) {
            let mut parts = Parts::default();
            let mut records = Vec::new();
            let mut full = 0;
            for (n, (subject, name)) in lens.into_iter().enumerate() {
                let subject = format!("{n:04}{}", "s".repeat(subject - 4));
                let new = record(&subject, &"r".repeat(name), None, None);
                let len = record_len(&new.reader);
                let used = used(&parts);
                let before: Vec<_> =
                    records.iter().map(|record| part(&parts, record)).collect();
                records.push(new.clone());
                if let Ok(added) = parts.place(&records, 0) {
                    prop_assert_eq!(added, [&new.reader]);
                    let at = usize::from(part(&parts, &new));
                    prop_assert!(used[at] + len <= PART_MAX);
                    let before = &used[..at];
                    prop_assert!(before.iter().all(|&used| used + len > PART_MAX));
                } else {
                    full += 1;
                    prop_assert!(used.iter().all(|&used| used + len > PART_MAX));
                    prop_assert!(!parts.keys.contains_key(&new.reader));
                    records.pop();
                }
                let after: Vec<_> = records[..before.len()]
                    .iter()
                    .map(|record| part(&parts, record))
                    .collect();
                prop_assert_eq!(after, before, "each record keeps its part");
            }
            prop_assert!(full == 0 || records.len() >= 448);
        }
    }
}
