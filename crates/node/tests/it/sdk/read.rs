//! The readers of the Rust SDK against a node process.

use std::sync::Arc;

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

/// A reader of channels of two indexes has the names at their places, then the index
/// of each, and gets the frames of each index in order.
#[test]
fn a_reader_of_two_indexes_gets_the_frames_of_each_in_order() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a", "cell.c", "plant.b"], Mode::Complete);
    let places = ["plant.a", "cell.c", "plant.b", "plant.time", "cell.time"];
    assert_eq!(names(&reader), places);
    let [a, c, b, plant, cell] = places.map(|name| at(reader.places(), name));
    let mut plants = writer(&client, &["plant.a", "plant.b"]);
    let mut cells = writer(&client, &["cell.c"]);
    let (wa, wb) = (
        at(plants.places(), "plant.a"),
        at(plants.places(), "plant.b"),
    );
    let wc = at(cells.places(), "cell.c");
    for n in 0..2 {
        let pair = [(wa, Value::I64(n)), (wb, Value::I64(n + 10))];
        assert_eq!(plants.write(&pair), Ok(taken(n.unsigned_abs(), 1)));
        let one = [(wc, Value::I64(n + 20))];
        assert_eq!(cells.write(&one), Ok(taken(n.unsigned_abs(), 1)));
    }
    let mut got = Vec::new();
    for _ in 0..4 {
        let frame = reader.next().unwrap();
        let index = frame.index();
        let samples = if index == plant {
            assert_eq!(frame.series(c), None);
            [a, b].map(|place| values(frame, place)[0]).to_vec()
        } else {
            assert_eq!(index, cell);
            vec![values(frame, c)[0]]
        };
        assert_eq!(frame.lost(a), 0..0);
        got.push((index, samples));
    }
    let of = |index| -> Vec<Vec<i64>> {
        let each = got.iter().filter(|(at, _)| *at == index);
        each.map(|(_, samples)| samples.clone()).collect()
    };
    assert_eq!(of(plant), [vec![0, 10], vec![1, 11]]);
    assert_eq!(of(cell), [vec![20], vec![21]]);
    assert_eq!(plants.close(), Ok(()));
    assert_eq!(cells.close(), Ok(()));
    drop(reader);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// Place `n` of a reader is the channel of name `n`, also when it comes before
/// another of its index or comes twice. Then the index of each index that no name
/// names, in the order of the first name of each index.
#[test]
fn a_place_of_a_reader_is_the_channel_of_its_name_then_each_index() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut swapped = reader(&client, &["plant.b", "plant.a"], Mode::Complete);
    assert_eq!(names(&swapped), ["plant.b", "plant.a", "plant.time"]);
    let mut twice = reader(&client, &["plant.a", "plant.a"], Mode::Complete);
    assert_eq!(names(&twice), ["plant.a", "plant.a", "plant.time"]);
    let mut cells = reader(&client, &["cell.c", "plant.a"], Mode::Complete);
    let indexes = ["cell.c", "plant.a", "cell.time", "plant.time"];
    assert_eq!(names(&cells), indexes);
    let mut plant = writer(&client, &["plant.a", "plant.b"]);
    let (wa, wb) = (at(plant.places(), "plant.a"), at(plant.places(), "plant.b"));
    let pair = [(wa, Value::I64(1)), (wb, Value::I64(2))];
    assert_eq!(plant.write(&pair), Ok(taken(0, 1)));
    let mut cell = writer(&client, &["cell.c"]);
    assert_eq!(cell.write(&[(0, Value::I64(3))]), Ok(taken(0, 1)));
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
    let mut got = Vec::new();
    for _ in 0..2 {
        let frame = cells.next().unwrap();
        let named = usize::from(frame.index() != 2);
        got.push((
            frame.index(),
            values(frame, named),
            frame.series(1 - named).is_none(),
        ));
    }
    got.sort_unstable();
    assert_eq!(got, [(2, vec![3], true), (3, vec![1], true)]);
    assert_eq!(plant.close(), Ok(()));
    assert_eq!(cell.close(), Ok(()));
    drop((swapped, twice, cells));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// The node bounds the streams of a session at 64, the hello one of them. With 61
/// streams held, a reader of two indexes opens in the two free streams.
#[test]
fn a_reader_of_two_indexes_opens_in_the_two_streams_that_its_session_has_free() {
    const HELD: usize = 61;
    let (mut rig, node, subject) = planted();
    let client = Arc::new(Client::connect(&node, subject).unwrap());
    let held: Vec<Reader> = (0..HELD)
        .map(|_| reader(&client, &["plant.a"], Mode::Complete))
        .collect();
    let opened = opened(&client, &["plant.a", "cell.c", "plant.b"]);
    let both = opened.unwrap();
    let places = ["plant.a", "cell.c", "plant.b", "plant.time", "cell.time"];
    assert_eq!(names(&both), places);
    drop((both, held));
    Arc::into_inner(client).unwrap().close().unwrap();
    rig.stop();
}

/// Opens a complete reader of `channels` on `client` on a thread of its own. Panics
/// when the open does not return within 30 s.
#[expect(
    clippy::disallowed_methods,
    reason = "the test bounds an open that can wait for a stream forever"
)]
fn opened(client: &Arc<Client>, channels: &[&str]) -> Result<Reader, Error> {
    let client = Arc::clone(client);
    let channels = channels.iter().map(|name| name.parse().unwrap()).collect();
    let (sender, answer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(sender.send(client.open_reader(read::Config::new(channels))));
    });
    super::wait(&answer)
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
    let mut reader = reader(&client, &["plant.a", "cell.c"], Mode::Complete);
    assert_eq!(client.close(), Ok(()));
    assert_eq!(reader.next().map(drop), Err(Error::Closed));
    assert_eq!(reader.next().map(drop), Err(Error::Closed));
    rig.stop();
}

/// Only Unix: `counting::Local` names the thread with `pthread_self`. A warm `next` of
/// a reader of two indexes makes no allocation on the caller's thread. A complete
/// reader gets each frame after its commit, so each call is one record of the ring,
/// which the node never trims (#160): about 2,000 calls fill it.
#[cfg(unix)]
#[test]
fn a_warm_next_makes_no_allocation_on_the_thread_of_its_caller() {
    const WARMUP: usize = 100;
    const CALLS: usize = 300;
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let mut reader = reader(&client, &["plant.a", "cell.c"], Mode::Complete);
    let mut plants = writer(&client, &["plant.a"]);
    let mut cells = writer(&client, &["cell.c"]);
    let mut call = || {
        for writer in [&mut plants, &mut cells] {
            let written = writer.write(&[(0, Value::I64(1))]);
            assert!(
                matches!(written, Ok(sdk::write::Outcome::Taken { .. })),
                "{written:?}"
            );
            let frame = reader.next().unwrap();
            assert_eq!(frame.count(), 1);
        }
    };
    (0..WARMUP).for_each(|_| call());
    let ((), allocations) = crate::ALLOCATOR.count(|| (0..CALLS).for_each(|_| call()));
    assert_eq!(allocations, 0, "{CALLS} warm calls of next allocate");
    assert_eq!(plants.close(), Ok(()));
    assert_eq!(cells.close(), Ok(()));
    drop(reader);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}
