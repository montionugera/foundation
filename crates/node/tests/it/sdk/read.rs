//! The readers of the Rust SDK against a node process.

use program::Refusal;
use sdk::read::{self, Frame, Mode, Reader};
use sdk::write::{Config, Writer};
use sdk::{Client, Error, Place};
use types::sample::Value;

use super::{planted, taken};

/// A reader of `channels` of `mode` on `client`.
fn reader(client: &Client, channels: &[&str], mode: Mode) -> Reader {
    let channels = channels.iter().map(|name| name.parse().unwrap()).collect();
    let config = read::Config { channels, mode };
    client.open_reader(config).unwrap()
}

/// A writer of `channels` on `client`.
fn writer(client: &Client, channels: &[&str]) -> Writer {
    let channels = channels.iter().map(|name| name.parse().unwrap()).collect();
    client.open_writer(Config::new(channels)).unwrap()
}

/// The names of the places of `reader`.
fn names(reader: &Reader) -> Vec<String> {
    reader.places().iter().map(|p| p.name.to_string()).collect()
}

/// The place of the channel `name` in `places`.
fn at(places: &[Place], name: &str) -> usize {
    let name = name.parse().unwrap();
    places.iter().position(|place| place.name == name).unwrap()
}

/// The samples of the series of `place` of `frame`, each as `i64`.
fn values(frame: &Frame, place: usize) -> Vec<i64> {
    let (chunks, rest) = frame.series(place).unwrap().as_chunks::<8>();
    assert!(rest.is_empty());
    chunks
        .iter()
        .map(|chunk| i64::from_le_bytes(*chunk))
        .collect()
}

/// A reader of one name gets each frame that a writer writes after the open, in
/// order, with the index as a place and nothing lost.
#[test]
fn a_reader_of_a_name_gets_the_frames_of_a_writer_in_order() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a"], Mode::Complete);
    assert_eq!(names(&reader), ["plant.a", "plant.time"]);
    let (time, a) = (
        at(reader.places(), "plant.time"),
        at(reader.places(), "plant.a"),
    );
    let mut writer = writer(&client, &["plant.a", "plant.b"]);
    let (wa, wb) = (
        at(writer.places(), "plant.a"),
        at(writer.places(), "plant.b"),
    );
    let values_ab = [(wa, Value::I64(1)), (wb, Value::I64(2))];
    assert_eq!(writer.write(&values_ab), Ok(taken(0, 1)));
    assert_eq!(writer.write(&[(wa, Value::I64(3))]), Ok(taken(1, 1)));
    let first = reader.next().unwrap();
    assert_eq!((first.count(), first.index()), (1, time));
    assert_eq!(values(first, a), [1]);
    assert_eq!(first.series(2), None);
    assert_eq!(first.lost(a), 0..0);
    let stamp = values(first, time)[0];
    let second = reader.next().unwrap();
    assert_eq!(values(second, a), [3]);
    assert!(values(second, time)[0] > stamp);
    assert_eq!(writer.close(), Ok(()));
    drop(reader);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// Place `n` of a reader is the channel of name `n`, also when it comes before
/// another of its index or comes twice. Then the index, when no name names it.
#[test]
fn a_place_of_a_reader_is_the_channel_of_its_name_then_its_index() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut swapped = reader(&client, &["plant.b", "plant.a"], Mode::Complete);
    assert_eq!(names(&swapped), ["plant.b", "plant.a", "plant.time"]);
    let mut twice = reader(&client, &["plant.a", "plant.a"], Mode::Complete);
    assert_eq!(names(&twice), ["plant.a", "plant.a", "plant.time"]);
    let mut named = reader(&client, &["plant.time", "plant.a"], Mode::Complete);
    assert_eq!(names(&named), ["plant.time", "plant.a"]);
    let mut plant = writer(&client, &["plant.a", "plant.b"]);
    let (wa, wb) = (at(plant.places(), "plant.a"), at(plant.places(), "plant.b"));
    let pair = [(wa, Value::I64(1)), (wb, Value::I64(2))];
    assert_eq!(plant.write(&pair), Ok(taken(0, 1)));
    let frame = swapped.next().unwrap();
    assert_eq!(
        (values(frame, 0), values(frame, 1), frame.index()),
        (vec![2], vec![1], 2)
    );
    let frame = twice.next().unwrap();
    assert_eq!(
        (values(frame, 0), values(frame, 1), frame.index()),
        (vec![1], vec![1], 2)
    );
    let frame = named.next().unwrap();
    assert_eq!((values(frame, 1), frame.index()), (vec![1], 0));
    assert_eq!(plant.close(), Ok(()));
    drop((swapped, twice, named));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// The node refuses a reader of channels of two indexes with `MALFORMED`, as it
/// refuses a writer, and the client goes on.
#[test]
fn an_open_of_a_reader_of_two_indexes_gives_malformed() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let channels = ["plant.a", "cell.c"].map(|name| name.parse().unwrap());
    let opened = client.open_reader(read::Config::new(channels.to_vec()));
    let malformed = Error::Stream(program::Error::Refused(Refusal::Malformed));
    assert_eq!(opened.map(drop), Err(malformed));
    drop(reader(&client, &["plant.a"], Mode::Complete));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// A latest reader gets a frame that a writer writes, with nothing lost.
#[test]
fn a_latest_reader_gets_a_frame_that_a_writer_writes() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a"], Mode::Latest);
    let a = at(reader.places(), "plant.a");
    let mut writer = writer(&client, &["plant.a"]);
    let wa = at(writer.places(), "plant.a");
    assert_eq!(writer.write(&[(wa, Value::I64(4))]), Ok(taken(0, 1)));
    let frame = reader.next().unwrap();
    assert_eq!(values(frame, a), [4]);
    assert_eq!(frame.lost(a), 0..0);
    assert_eq!(writer.close(), Ok(()));
    drop(reader);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// A name of no channel gives `UNKNOWN`, alone and beside a name of a channel, and
/// the client goes on.
#[test]
fn an_open_of_a_reader_of_an_unknown_name_gives_unknown() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let unknown = Error::Stream(program::Error::Refused(Refusal::Unknown));
    for channels in [&["plant.missing"][..], &["plant.a", "plant.missing"]] {
        let channels = channels.iter().map(|name| name.parse().unwrap()).collect();
        let opened = client.open_reader(read::Config::new(channels)).map(drop);
        assert_eq!(opened, Err(unknown.clone()));
    }
    drop(reader(&client, &["plant.a"], Mode::Complete));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// The close of a client with a live reader returns, and each later call of the
/// reader gives `Closed`.
#[test]
fn a_close_of_a_client_with_a_live_reader_returns_and_ends_the_reader() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a"], Mode::Complete);
    assert_eq!(client.close(), Ok(()));
    assert_eq!(reader.next().map(drop), Err(Error::Closed));
    assert_eq!(reader.next().map(drop), Err(Error::Closed));
    rig.stop();
}

/// Asserts that a warm `next` makes no allocation on the caller's thread. A complete
/// reader gets each frame after its commit, so each call is one record of the ring,
/// which the node never trims (#160): about 4,000 calls fill it.
#[cfg(unix)]
pub(super) fn a_warm_next_makes_no_allocation() {
    const WARMUP: usize = 100;
    const CALLS: usize = 300;
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a"], Mode::Complete);
    let mut writer = writer(&client, &["plant.a"]);
    let mut call = || {
        let written = writer.write(&[(0, Value::I64(1))]);
        assert!(
            matches!(written, Ok(sdk::write::Outcome::Taken { .. })),
            "{written:?}"
        );
        let frame = reader.next().unwrap();
        assert_eq!(frame.count(), 1);
    };
    (0..WARMUP).for_each(|_| call());
    let ((), allocations) = crate::ALLOCATOR.count(|| (0..CALLS).for_each(|_| call()));
    assert_eq!(allocations, 0, "{CALLS} warm calls of next allocate");
    assert_eq!(writer.close(), Ok(()));
    drop(reader);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}
