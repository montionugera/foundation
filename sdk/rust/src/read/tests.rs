// These tests serve the calls of a reader on a thread of their own, in place of the
// task of the client's thread, whose `program::Reader` needs a node. The tests of
// `crates/node/tests/it/sdk/read.rs` run the task against a node process.

use std::future::Pending;
use std::task::{Context, Waker};
use std::thread::{self, JoinHandle};

use codec::Encoder;
use types::channel;
use types::frame::key_set::{Group, Interner};
use types::frame::{Draft, Form, Path};
use types::sample::{Scalar, Type};

use super::*;

/// Runs `serve` on a thread of its own, in place of the client's thread.
#[expect(
    clippy::disallowed_methods,
    reason = "a thread stands in for the client's thread"
)]
fn spawn<T: Send + 'static>(
    serve: impl FnOnce() -> T + Send + 'static,
) -> JoinHandle<T> {
    thread::spawn(serve)
}

/// The place of the channel of key `at`, of `scalar` samples, named `name`.
fn place(at: u128, scalar: Scalar, name: &str) -> Place {
    Place {
        key: channel::Key::from_u128(at),
        data_type: Type::Scalar(scalar),
        name: name.parse().unwrap(),
    }
}

/// `count` (`i64`), then `time`, its index.
fn places() -> Vec<Place> {
    vec![
        place(2, Scalar::I64, "count"),
        place(1, Scalar::Stamp, "time"),
    ]
}

fn config() -> Config {
    Config::new(vec!["count".parse().unwrap()])
}

/// A frame of `count` samples whose index is place 1, with a series of each of
/// `series`: its place, its bytes, and its lost seqs.
fn frame(count: u32, series: &[(usize, &[u8], Range<u64>)]) -> Frame {
    let series: Vec<Series> = series
        .iter()
        .map(|(place, bytes, lost)| Series {
            place: *place,
            bytes: bytes.to_vec(),
            lost: lost.clone(),
        })
        .collect();
    Frame {
        count,
        index: 1,
        len: series.len(),
        series,
    }
}

/// A reader of `places()` whose calls a thread serves with `answer`, which takes the
/// count of each call and the frame of the call, and gives the outcome. The thread
/// gives the count of the calls that it served.
fn reader(
    mut answer: impl FnMut(usize, &mut Frame) -> Result<(), Error> + Send + 'static,
) -> (Reader, JoinHandle<usize>) {
    let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
    let served = spawn(move || {
        let Open {
            opened: answer_open,
            mut calls,
            read,
            ..
        } = opened.blocking_recv().unwrap();
        answer_open.send(Ok(places())).unwrap();
        let mut served = 0;
        while let Some(mut frame) = calls.blocking_recv() {
            let outcome = answer(served, &mut frame);
            served += 1;
            read.send((outcome, frame)).unwrap();
        }
        served
    });
    (
        Reader::open(&opens, &Arc::default(), config()).unwrap(),
        served,
    )
}

#[test]
fn a_config_of_new_is_complete() {
    let channels = vec!["count".parse().unwrap()];
    let config = Config::new(channels.clone());
    assert_eq!(
        config,
        Config {
            channels,
            mode: Mode::Complete
        }
    );
}

#[test]
fn a_reader_moves_between_threads() {
    fn sent<T: Send>() {}
    sent::<Reader>();
}

#[test]
fn an_open_sends_its_config_and_gives_the_places_of_the_node() {
    let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
    let served = spawn(move || {
        let open = opened.blocking_recv().unwrap();
        open.opened.send(Ok(places())).unwrap();
        open.config
    });
    let config = Config {
        channels: vec!["count".parse().unwrap(), "level".parse().unwrap()],
        mode: Mode::Latest,
    };
    let reader = Reader::open(&opens, &Arc::default(), config.clone()).unwrap();
    assert_eq!(reader.places(), places());
    assert_eq!(served.join().unwrap(), config);
}

#[test]
fn an_open_gives_the_error_of_the_thread() {
    let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
    let served = spawn(move || {
        let open = opened.blocking_recv().unwrap();
        let refused = Error::Stream(program::Error::Unanswered);
        open.opened.send(Err(refused)).unwrap();
    });
    let opened = Reader::open(&opens, &Arc::default(), config()).map(drop);
    assert_eq!(opened, Err(Error::Stream(program::Error::Unanswered)));
    served.join().unwrap();
}

#[test]
fn an_open_of_no_channel_gives_unnamed_and_sends_nothing() {
    let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
    let none = Config::new(Vec::new());
    let opened_none = Reader::open(&opens, &Arc::default(), none).map(drop);
    assert_eq!(opened_none, Err(Error::Stream(program::Error::Unnamed)));
    assert_eq!(
        Error::Stream(program::Error::Unnamed).to_string(),
        "the stream names no channel"
    );
    opened.try_recv().unwrap_err();
}

/// The flag of a client that closed or dropped, and the error of a call whose task
/// ended, then of a flag of a client that did not.
const LOST: [(bool, Error); 2] = [(true, Error::Closed), (false, Error::Panicked)];

#[test]
fn an_open_whose_thread_ended_gives_closed_after_a_close_else_panicked() {
    for (closed, error) in LOST {
        let closed = Arc::new(AtomicBool::new(closed));
        let (opens, opened) = mpsc::unbounded_channel::<Open>();
        drop(opened);
        let open = Reader::open(&opens, &closed, config()).map(drop);
        assert_eq!(open, Err(error.clone()));
        let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
        let served = spawn(move || drop(opened.blocking_recv()));
        assert_eq!(
            Reader::open(&opens, &closed, config()).map(drop),
            Err(error)
        );
        served.join().unwrap();
    }
}

#[test]
fn a_call_gives_the_frame_of_the_thread_and_the_next_call_sends_it_back() {
    let (mut reader, served) = reader(|call, frame| {
        assert_eq!(frame.count, u32::try_from(call).unwrap());
        *frame = self::frame(frame.count + 1, &[(0, &[7; 8], 2..4)]);
        Ok(())
    });
    let first = reader.next().unwrap();
    assert_eq!((first.count(), first.index()), (1, 1));
    assert_eq!((first.series(0), first.lost(0)), (Some(&[7; 8][..]), 2..4));
    assert_eq!(reader.next().unwrap().count(), 2);
    drop(reader);
    assert_eq!(served.join().unwrap(), 2);
}

#[test]
fn an_error_ends_the_reader_and_each_later_call_gives_it_with_nothing_sent() {
    let failed = Error::Stream(program::Error::Unanswered);
    let error = failed.clone();
    let (mut reader, served) = reader(move |_, _| Err(error.clone()));
    assert_eq!(reader.next().map(drop), Err(failed.clone()));
    assert_eq!(reader.next().map(drop), Err(failed));
    drop(reader);
    assert_eq!(served.join().unwrap(), 1);
}

#[test]
fn each_call_of_a_reader_whose_task_ended_gives_closed_after_a_close_else_panicked() {
    for (closed, error) in LOST {
        let (opens, mut opened) = mpsc::unbounded_channel::<Open>();
        let served = spawn(move || {
            let Open {
                opened: answer,
                mut calls,
                ..
            } = opened.blocking_recv().unwrap();
            answer.send(Ok(places())).unwrap();
            drop(calls.blocking_recv());
        });
        let closed = Arc::new(AtomicBool::new(closed));
        let mut reader = Reader::open(&opens, &closed, config()).unwrap();
        assert_eq!(reader.next().map(drop), Err(error.clone()));
        served.join().unwrap();
        assert_eq!(reader.next().map(drop), Err(error));
    }
}

#[test]
fn a_frame_gives_only_its_series_and_its_first_stamp() {
    let stamps = [5_i64, 6].map(i64::to_le_bytes).concat();
    let mut frame = frame(2, &[(1, &stamps, 0..0), (0, &[0; 16], 3..9)]);
    assert_eq!(frame.series(1), Some(stamps.as_slice()));
    assert_eq!(
        (frame.lost(0), frame.lost(1), frame.lost(2)),
        (3..9, 0..0, 0..0)
    );
    assert_eq!(frame.first(), Some(5));
    frame.len = 1;
    assert_eq!((frame.series(0), frame.lost(0)), (None, 0..0));
    frame.len = 0;
    assert_eq!(frame.first(), None);
    let empty = self::frame(0, &[(1, &[], 0..0)]);
    assert_eq!(empty.first(), None);
}

#[test]
fn the_earliest_is_a_frame_of_no_sample_then_the_earliest_stamp_then_the_first() {
    type Held = Option<Option<i64>>;
    let cases: [(&[Held], Option<usize>); 6] = [
        (&[], None),
        (&[None, None], None),
        (&[None, Some(Some(3))], Some(1)),
        (&[Some(Some(4)), Some(Some(3)), Some(Some(5))], Some(1)),
        (&[Some(Some(3)), None, Some(Some(3))], Some(0)),
        (&[Some(Some(-1)), Some(None), Some(None)], Some(1)),
    ];
    for (firsts, at) in cases {
        assert_eq!(earliest(firsts.iter().copied()), at, "{firsts:?}");
    }
}

proptest::proptest! {
    /// The earliest is the first held frame that no held frame before it ties and no
    /// held frame comes before: a frame of no sample before each stamp.
    #[test]
    fn the_earliest_comes_before_each_other_held_frame(
        firsts in proptest::collection::vec(
            proptest::option::of(proptest::option::of(-3_i64..3)),
            0..6,
        ),
    ) {
        let at = earliest(firsts.iter().copied());
        let held: Vec<_> = (0..firsts.len()).filter(|&i| firsts[i].is_some()).collect();
        proptest::prop_assert_eq!(at.is_none(), held.is_empty());
        if let Some(at) = at {
            let first = firsts[at].expect("the earliest is held");
            for &other in &held {
                let theirs = firsts[other].expect("held");
                let before = match (theirs, first) {
                    (None, Some(_)) => true,
                    (Some(theirs), Some(first)) => theirs < first,
                    _ => false,
                };
                proptest::prop_assert!(!before, "{other} comes before {at}");
                if theirs == first {
                    proptest::prop_assert!(at <= other, "{at} ties {other} after it");
                }
            }
        }
    }
}

/// The merge of reads of the channels of `merged`, with no read to drive.
fn merged(merged: &mut [Merged], frame: &mut Frame) -> Poll<Result<(), Error>> {
    let mut cx = Context::from_waker(Waker::noop());
    let parts: &mut [Option<Pin<Box<Pending<()>>>>] = &mut [];
    merge(&mut cx, parts, merged, frame)
}

#[test]
fn a_merge_gives_the_earliest_frame_and_frees_the_frame_of_the_call_to_its_read() {
    let mut reads = Vec::new();
    let mut sides = Vec::new();
    for _ in 0..2 {
        let (given, taken) = mpsc::channel(1);
        let (free, freed) = mpsc::channel(1);
        reads.push(Merged {
            taken,
            free,
            held: None,
        });
        sides.push((given, freed));
    }
    let mut call = frame(9, &[]);
    assert_eq!(merged(&mut reads, &mut call), Poll::Pending);
    sides[0].0.try_send((Ok(Some(7)), frame(7, &[]))).unwrap();
    sides[1].0.try_send((Ok(Some(5)), frame(5, &[]))).unwrap();
    assert_eq!(merged(&mut reads, &mut call), Poll::Ready(Ok(())));
    assert_eq!(call.count, 5);
    assert_eq!(sides[1].1.try_recv().unwrap().count, 9);
    sides[0].1.try_recv().unwrap_err();
    assert_eq!(merged(&mut reads, &mut call), Poll::Ready(Ok(())));
    assert_eq!(call.count, 7);
    assert_eq!(sides[0].1.try_recv().unwrap().count, 5);
    assert_eq!(merged(&mut reads, &mut call), Poll::Pending);
    let failed = Error::Stream(program::Error::Unanswered);
    sides[1]
        .0
        .try_send((Err(failed.clone()), frame(1, &[])))
        .unwrap();
    assert_eq!(merged(&mut reads, &mut call), Poll::Ready(Err(failed)));
    assert_eq!(call.count, 7);
}

/// A frame of a reader of `time`, entry 0, and `count`, entry 1, as the node gives
/// it: each of `series` is an entry and its raw samples, which it encodes, or its
/// bytes as they are when `raw` is false.
fn read(samples: u32, series: &[(usize, &[u8], bool)]) -> types::frame::Frame {
    let key = channel::Key::from_u128;
    let group = Group {
        index: key(1),
        data: &[(key(2), Type::Scalar(Scalar::I64))],
    };
    let set = Interner::new().intern(&[group]);
    let types = [Scalar::Stamp, Scalar::I64].map(Type::Scalar);
    let bytes: Vec<(usize, Vec<u8>)> = series
        .iter()
        .map(|&(entry, values, raw)| {
            if !raw {
                return (entry, values.to_vec());
            }
            let mut out = vec![0; codec::max_len(types[entry], values.len())];
            let count = usize::try_from(samples).unwrap();
            let len = Encoder::new(types[entry]).encode(count, values, &mut out);
            out.truncate(len.unwrap());
            (entry, out)
        })
        .collect();
    let lens: Vec<(usize, usize)> =
        bytes.iter().map(|(at, b)| (*at, b.len())).collect();
    let config = block::Config::new(1 << 16).unwrap();
    let memory = block::Heap::new(config.reservation());
    let pool = block::Pool::new(config, memory);
    let mut draft = Draft::new(&pool, &set, Form::Encoded, &lens).unwrap();
    for (entry, series) in &bytes {
        draft.series_mut(*entry).unwrap().copy_from_slice(series);
    }
    draft.set_count(0, samples);
    draft.freeze(Path::Live)
}

#[test]
fn a_decode_gives_each_series_raw_with_its_place_and_its_lost_seqs() {
    let places = [
        place(1, Scalar::Stamp, "time"),
        place(2, Scalar::I64, "count"),
    ];
    let stamps = [5_i64, 6].map(i64::to_le_bytes).concat();
    let counts = [-1_i64, 9].map(i64::to_le_bytes).concat();
    let lost = |entry| if entry == 1 { 4..6 } else { 0..0 };
    let mut frame = frame(9, &[(0, &[1; 3], 1..2), (1, &[], 0..0), (2, &[], 0..0)]);
    let both = read(2, &[(0, &stamps, true), (1, &counts, true)]);
    assert_eq!(decoded(&places, 3, lost, 3, &both, &mut frame), Ok(Some(5)));
    assert_eq!((frame.count(), frame.index(), frame.len), (2, 3, 2));
    assert_eq!((frame.series(3), frame.lost(3)), (Some(&stamps[..]), 0..0));
    assert_eq!((frame.series(4), frame.lost(4)), (Some(&counts[..]), 4..6));
    let index = read(1, &[(0, &stamps[..8], true)]);
    assert_eq!(
        decoded(&places, 3, lost, 3, &index, &mut frame),
        Ok(Some(5))
    );
    assert_eq!((frame.count(), frame.len, frame.series(4)), (1, 1, None));
}

#[test]
fn a_series_that_does_not_decode_gives_decode_with_its_place() {
    let places = [
        place(1, Scalar::Stamp, "time"),
        place(2, Scalar::I64, "count"),
    ];
    let stamps = 5_i64.to_le_bytes();
    let bad = [0xff; 8];
    let mut out = Vec::new();
    let i64 = Type::Scalar(Scalar::I64);
    let cause = codec::decode(i64, 1, &bad, &mut out).unwrap_err();
    let broken = read(1, &[(0, &stamps, true), (1, &bad, false)]);
    let mut frame = Frame::default();
    let decoded = decoded(&places, 0, |_| 0..0, 2, &broken, &mut frame);
    assert_eq!(decoded, Err(Error::Decode { place: 3, cause }));
}
