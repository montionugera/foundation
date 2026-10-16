//! The named entry of an index. Its body holds the record of each named reader of the
//! index, open or held, from the entry's place in the index log. Each entry replaces
//! the ones before it.

use std::num::NonZeroU8;
use std::str;

use block::Block;
use buffer::{Buffer, Entry, Layout};
use delivery::{Position, Record};
use types::channel::{self, Slot};
use types::frame::Path;
use types::name::Name;
use types::time::{Span, Stamp};

use crate::state;

/// The buffer tag of a named entry.
const TAG: NonZeroU8 = NonZeroU8::new(2).expect("2 is not 0");

/// The version of the body that this build writes and reads.
const VERSION: u8 = 1;

/// The bytes of the body before the records: the version and the count.
const HEAD: usize = 1 + 4;

/// The bytes of a record apart from its subject and name.
const FIXED: usize = 1 + 1 + 8 + 1 + 8 + 8 + 1 + 8;

/// The most bytes of one record: the longest subject and name.
pub(crate) const RECORD_MAX: usize = FIXED + 2 * Name::MAX_BYTES;
const _: () = assert!(
    HEAD + 7 * RECORD_MAX <= Layout::ENTRY_MAX_MIN,
    "7 records fit one entry of any ring"
);

/// The most bytes of the body of a named entry in `buffer`: of one block, and of one
/// entry of its ring.
pub(crate) fn limit(buffer: &Buffer) -> usize {
    buffer.block_max().min(buffer.layout().entry_max())
}

/// The bytes of the body that holds `records`.
pub(crate) fn len<'a>(records: impl IntoIterator<Item = &'a Record>) -> usize {
    let records = records.into_iter().map(|record| record_len(&record.reader));
    records.sum::<usize>() + HEAD
}

/// The bytes of the record of `reader` in a body.
pub(crate) fn record_len(reader: &delivery::named::Key) -> usize {
    FIXED + reader.subject.as_str().len() + reader.name.as_str().len()
}

/// The buffer entry that holds `records` on the live path of the index `index` at
/// `slot`, at seq `first` and mesh time `stored_at`. Its body is a block from `pool`.
///
/// # Errors
///
/// [`block::Error`] when `pool` has no block for the bytes.
///
/// # Panics
///
/// If `records` hold more than `u32::MAX` records.
pub(crate) fn entry(
    pool: &block::Pool,
    records: &[Record],
    index: channel::Key,
    slot: Slot,
    first: u64,
    stored_at: Stamp,
) -> Result<Entry, block::Error> {
    let body = Some(body(pool, records)?);
    Ok(state::entry(TAG, body, index, slot, first, stored_at))
}

/// The body that holds `records`, a block from `pool`.
///
/// # Errors
///
/// [`block::Error`] when `pool` has no block for the bytes.
fn body(pool: &block::Pool, records: &[Record]) -> Result<Block, block::Error> {
    let mut block = pool.alloc(len(records))?;
    write(&mut block, records);
    Ok(block.freeze())
}

/// Writes `records` to `out`, which has [`len`] bytes.
fn write(out: &mut [u8], records: &[Record]) {
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

/// The records that the newest durable named entry of each index in `buffer` holds,
/// with the index's slot, in no order. An index with no named entry is absent.
///
/// # Errors
///
/// The error of [`Buffer::newest`].
///
/// # Panics
///
/// When a named entry does not decode, as [`read`] says.
pub(crate) async fn newest(
    buffer: &Buffer,
) -> Result<Vec<(Slot, Vec<Record>)>, buffer::Error> {
    let found = buffer.newest(Path::Live, TAG).await?;
    let records = found
        .into_iter()
        .map(|(slot, stored)| (slot, read(&stored.bytes)));
    Ok(records.collect())
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
    use crate::common::{create_pool, key};

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
        body(&create_pool(1 << 16), records).expect("room").to_vec()
    }

    mod entry {
        use super::*;

        #[test]
        fn holds_the_records_on_the_live_path_with_tag_2() {
            let records = [record("ops", "r", None, None)];
            let stored_at = Stamp::from_nanos(9);

            let entry = entry(
                &create_pool(4096),
                &records,
                key(Slot::new(4)),
                Slot::new(4),
                12,
                stored_at,
            )
            .expect("room");

            let place = (entry.index, entry.slot, entry.path, entry.first);
            assert_eq!(place, (key(Slot::new(4)), Slot::new(4), Path::Live, 12));
            assert_eq!(
                (entry.samples, entry.stored_at, entry.tag),
                (None, stored_at, 2)
            );
            let parts: Vec<_> =
                entry.parts.into_iter().map(|part| part.to_vec()).collect();
            assert_eq!(parts, [encoded(&records)]);
        }
    }

    mod body {
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
            assert_eq!((RECORD_MAX, len(&records)), (546, 551));
        }

        #[test]
        fn returns_the_pool_error_when_the_pool_is_full() {
            let pool = create_pool(4096);
            let mut held = Vec::new();
            while let Ok(block) = pool.alloc(4) {
                held.push(block);
            }
            let available = 4096 - pool.committed();

            let error = body(&pool, &[]).expect_err("full");

            assert_eq!(
                error,
                block::Error::Exhausted {
                    requested: 5,
                    available,
                }
            );
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
}
