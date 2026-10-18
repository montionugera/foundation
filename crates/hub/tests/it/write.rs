//! The write streams of a client session that `Link::serve` serves, over a real
//! transport from a program to a simulated node.

use std::cell::Cell;
use std::path::{Path as FilePath, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use access::proof::{Error as Refusal, Kind};
use block::Unique;
use env::files::Operation;
use hub::{serve, writer};
use transport::Class;
use types::authority::Authority;
use types::channel;
use types::frame::{Form, Path};
use types::time::Span;
use wire::header::MALFORMED;
use wire::hub::client::write::Open;
use wire::hub::client::{BODY_BYTES_MAX, Mode, Place, Places, REFUSED};
use wire::hub::write::{Head, Outcome, Reply, Time};
use wire::hub::{BUSY, FAILED, NOT_HOME, UNKNOWN};

use super::link::as_agent;
use super::read::{finish, read_session};
use super::serve::decoded;
use super::{
    AREA, BODY_MAX, I64, NODE, POOL, RING, STAMP, TIME, TIME_B, Test, channels, region,
    without,
};
use crate::agent::{AGENT, Agent, SUBJECT, Stream, name, reset_with};
use crate::net::{HOME, own_pool, transport};
use crate::sessions::{Got, Kept, OTHER, recorded, rules, run_program, serve_each};

/// When the home of a test changes what the write meets, after the program opened it.
const START: Span = Span::from_nanos(100_000_000);

fn place(key: u128, data_type: types::sample::Type, channel: &str) -> Place {
    Place {
        key: channel::Key::from_u128(key),
        data_type,
        name: name(channel),
    }
}

/// The head of a frame of `count` samples of each of `series` series on the live
/// path.
fn live(count: u32, series: u32) -> Head {
    Head {
        time: Time::Frame(Path::Live),
        form: Form::Raw,
        count,
        series,
    }
}

/// The head of a frame of one sample of each of `series` series that the node stamps.
fn untimed(series: u32) -> Head {
    Head {
        time: Time::Node,
        form: Form::Raw,
        count: 1,
        series,
    }
}

/// The ends and the body of a frame of `series`, each a place and its samples.
fn frame(series: &[(u32, &[i64])]) -> (Vec<(u32, u32)>, Vec<u8>) {
    let mut body = Vec::new();
    let ends = series
        .iter()
        .map(|&(place, samples)| {
            body.extend(samples.iter().flat_map(|sample| sample.to_le_bytes()));
            (place, u32::try_from(body.len()).expect("a body fits a u32"))
        })
        .collect();
    (ends, body)
}

/// Sends a live frame of `stamps` at place 0 and `values` at place 1.
async fn send(stream: &mut Stream, stamps: &[i64], values: &[i64]) {
    let count = u32::try_from(stamps.len()).expect("a count");
    let (ends, body) = frame(&[(0, stamps), (1, values)]);
    stream.frame(live(count, 2), &ends, &body).await;
}

/// Sends a frame of `value` at place 1 that the node stamps.
async fn send_untimed(stream: &mut Stream, value: i64) {
    let (ends, body) = frame(&[(0, &[0]), (1, &[value])]);
    stream.frame(untimed(2), &ends, &body).await;
}

/// Finishes `stream`, and gives each reply that the node sent before it finished.
async fn end(stream: &mut Stream) -> Vec<Reply> {
    stream.sender.finish().expect("finishes");
    let mut replies = Vec::new();
    while let Some(message) = stream.recv().await.expect("receives") {
        replies.push(Reply::decode(&message).expect("a reply"));
    }
    replies
}

fn stored(through: u64) -> Reply {
    Reply::Stored { through }
}

fn taken(seq: u64, count: u32) -> Reply {
    Reply::Outcome(Outcome::Taken {
        range: types::frame::Range { seq, count },
    })
}

fn refused(refusal: wire::hub::write::Refusal) -> Reply {
    Reply::Outcome(Outcome::Refused(refusal))
}

/// Opens a write of `value`, and checks its places: `time` at place 0 and `value` at
/// place 1.
async fn write(agent: &Agent) -> Stream {
    let mut stream = agent.write(&["value"]).await;
    let start = stream.started().await;
    assert_eq!(
        start.places,
        [place(1, STAMP, "time"), place(2, I64, "value")]
    );
    stream
}

/// Opens a complete read of `value`, whose places are `value`, then `time`.
async fn read(agent: &Agent) -> (Stream, wire::hub::Reader) {
    let mut stream = agent.read(Mode::Complete, &["value"]).await;
    let (places, _, reader) = stream.opened().await;
    assert_eq!(places, Places { count: 2, index: 1 });
    (stream, reader)
}

/// The values and the stamps of the next frame of a read of `value`.
async fn next(stream: &mut Stream, reader: &mut wire::hub::Reader) -> (i64, i64) {
    let got = super::serve::got(&mut stream.receiver, reader).await;
    let series = decoded(&got, &[I64, STAMP]);
    (series[0][0], series[1][0])
}

/// Each frame of a write is taken in order, a read at the home gets it, and `Stored`
/// comes after its commit with its seq.
#[test]
fn takes_each_frame_of_a_write_in_order_and_stores_it() {
    let home = read_session(
        400,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let (mut reading, mut reader) = read(&agent).await;
            let mut stream = write(&agent).await;
            for (seq, (stamp, value)) in (0..).zip([(1, 10), (2, 20)]) {
                send(&mut stream, &[stamp], &[value]).await;
                assert_eq!(stream.reply().await, taken(seq, 1));
                assert_eq!(stream.reply().await, Reply::Stored { through: seq });
                assert_eq!(next(&mut reading, &mut reader).await, (value, stamp));
            }
            stream.sender.finish().expect("finishes");
            assert_eq!(stream.recv().await, Ok(None));
            reading.sender.finish().expect("finishes");
            assert_eq!(reading.recv().await, Ok(None));
            finish(&mut agent).await;
        },
    );
    assert_eq!(
        home.served,
        [Ok(Got::Ended), Ok(Got::Ended), Ok(Got::Ended)]
    );
}

/// A program that finishes its stream right after a frame gets the frame's `Stored`
/// before the finish.
#[test]
fn stores_each_taken_frame_before_it_finishes_the_stream() {
    let home = read_session(
        401,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut stream = write(&agent).await;
            send(&mut stream, &[1, 2, 3], &[10, 20, 30]).await;
            stream.sender.finish().expect("finishes");
            assert_eq!(stream.reply().await, taken(0, 3));
            assert_eq!(stream.reply().await, Reply::Stored { through: 2 });
            assert_eq!(stream.recv().await, Ok(None));
            finish(&mut agent).await;
        },
    );
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// A write is served on a stream of each class, as the class sets only the priority
/// of its replies.
#[test]
fn serves_a_write_on_a_stream_of_each_class() {
    let home = read_session(
        416,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let classes = [Class::Command, Class::Latest, Class::CatchUp];
            for (seq, class) in (0..).zip(classes) {
                let mut stream = agent.write_as(class, &["value"]).await;
                stream.started().await;
                let stamp = i64::try_from(seq).expect("a stamp") + 1;
                send(&mut stream, &[stamp], &[10]).await;
                assert_eq!(stream.reply().await, taken(seq, 1), "{class:?}");
                assert_eq!(end(&mut stream).await, [stored(seq)], "{class:?}");
            }
            finish(&mut agent).await;
        },
    );
    assert_eq!(
        home.served,
        [
            Ok(Got::Ended),
            Ok(Got::Ended),
            Ok(Got::Ended),
            Ok(Got::Ended)
        ]
    );
}

/// The node stamps a frame of `Time::Node` with mesh time, and a frame at the same
/// mesh time 1 ns after the stamp before it. A frame after mesh time moves on gets
/// mesh time again.
#[test]
fn stamps_an_untimed_frame_with_mesh_time_and_after_the_stamp_before_it() {
    let home = read_session(
        402,
        async |_| {},
        |mut agent| async move {
            let challenge = agent.admit().await;
            let (mut reading, mut reader) = read(&agent).await;
            let mut stream = write(&agent).await;
            send_untimed(&mut stream, 10).await;
            send_untimed(&mut stream, 20).await;
            assert_eq!(stream.reply().await, taken(0, 1));
            assert_eq!(stream.reply().await, taken(1, 1));
            let (value, first) = next(&mut reading, &mut reader).await;
            assert_eq!(value, 10);
            assert!(
                first >= challenge.now.earliest.nanos()
                    && first < challenge.now.latest.nanos() + Span::SECOND.nanos(),
                "the stamp {first} is not mesh time after {:?}",
                challenge.now
            );
            assert_eq!(next(&mut reading, &mut reader).await, (20, first + 1));
            agent.sleep(Span::MILLISECOND).await;
            send_untimed(&mut stream, 30).await;
            assert_eq!(stream.reply().await, stored(0));
            assert_eq!(stream.reply().await, stored(1));
            assert_eq!(stream.reply().await, taken(2, 1));
            let (value, third) = next(&mut reading, &mut reader).await;
            assert_eq!(value, 30);
            assert!(
                third >= first + Span::MILLISECOND.nanos(),
                "the stamp {third} is not mesh time 1 ms after {first}"
            );
            assert_eq!(end(&mut stream).await, [stored(2)]);
            reading.sender.finish().expect("finishes");
            assert_eq!(reading.recv().await, Ok(None));
            finish(&mut agent).await;
        },
    );
    assert_eq!(
        home.served,
        [Ok(Got::Ended), Ok(Got::Ended), Ok(Got::Ended)]
    );
}

/// A frame of `Time::Node` behind a later stamp of another writer, and a timed frame
/// before the last stamp of the node, are refused, and the stream takes the next
/// frame.
#[test]
fn refuses_a_frame_out_of_order_and_takes_the_next() {
    let ready = async |test: &Test| {
        let mut writer = test.writer("a", &["value"]).await;
        let later = test.now() + Span::from_nanos(500_000_000).nanos();
        super::write(&mut writer, &[later], &[1]);
    };
    let home = read_session(403, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send_untimed(&mut stream, 10).await;
        assert_eq!(
            stream.reply().await,
            refused(wire::hub::write::Refusal::Order)
        );
        agent.sleep(Span::from_nanos(600_000_000)).await;
        send_untimed(&mut stream, 20).await;
        assert_eq!(stream.reply().await, taken(1, 1));
        send(&mut stream, &[5], &[30]).await;
        assert_eq!(
            stream.reply().await,
            refused(wire::hub::write::Refusal::Order)
        );
        send_untimed(&mut stream, 40).await;
        assert_eq!(stream.reply().await, taken(2, 1));
        assert_eq!(end(&mut stream).await, [stored(1), stored(2)]);
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// A frame of `Time::Node` that the home loses for no room in the ring spends its
/// stamp, so the next one at the same mesh time gets 1 ns after it and is not refused
/// as `Order`.
#[test]
fn stamps_an_untimed_frame_after_a_lost_untimed_frame() {
    let home = read_session(
        420,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut stream = write(&agent).await;
            for n in 1.. {
                let stamps: Vec<i64> = (n * 1000..(n + 1) * 1000).collect();
                send(&mut stream, &stamps, &noise(&stamps)).await;
                match outcome(&mut stream).await {
                    Outcome::Taken { .. } => {}
                    Outcome::Lost { .. } => break,
                    refused @ Outcome::Refused(_) => panic!("frame {n}: {refused:?}"),
                }
            }
            send_untimed(&mut stream, 10).await;
            send_untimed(&mut stream, 20).await;
            let got = [outcome(&mut stream).await, outcome(&mut stream).await];
            let [
                Outcome::Lost { range: first },
                Outcome::Lost { range: second },
            ] = got
            else {
                panic!("{got:?}");
            };
            assert_eq!(second.seq, first.seq + 1);
            end(&mut stream).await;
            finish(&mut agent).await;
        },
    );
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// A frame whose series do not hold its count is refused as `Codec`: one of
/// `Time::Node` whose index is not one stamp, before the node stamps it, and a timed
/// one at the home. The stream takes the next frame.
#[test]
fn refuses_a_frame_whose_series_do_not_hold_its_count_as_codec() {
    let home = read_session(
        421,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut stream = write(&agent).await;
            let codec = refused(wire::hub::write::Refusal::Codec);
            for stamps in [&[][..], &[0, 0]] {
                let (ends, body) = frame(&[(0, stamps), (1, &[10])]);
                stream.frame(untimed(2), &ends, &body).await;
                assert_eq!(stream.reply().await, codec, "{stamps:?}");
            }
            let (ends, body) = frame(&[(0, &[1, 2]), (1, &[10])]);
            stream.frame(live(1, 2), &ends, &body).await;
            assert_eq!(stream.reply().await, codec);
            send_untimed(&mut stream, 20).await;
            assert_eq!(stream.reply().await, taken(0, 1));
            assert_eq!(end(&mut stream).await, [stored(0)]);
            finish(&mut agent).await;
        },
    );
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// A write whose index another writer controls is refused at each frame as `Waiting`.
#[test]
fn refuses_a_frame_while_another_writer_holds_control() {
    let ready = async |test: &Test| {
        let writer = test.writer("a", &["value"]).await;
        let clock = test.clock.clone();
        test.tasks.spawn(async move {
            clock
                .sleep(Span::from_nanos(10 * Span::SECOND.nanos()))
                .await;
            drop(writer);
        });
    };
    let home = read_session(404, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(
            stream.reply().await,
            refused(wire::hub::write::Refusal::Waiting)
        );
        assert_eq!(end(&mut stream).await, []);
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// Every block of `pool` from the largest down to blocks of `least` bytes, but the last
/// one, so the pool has room for small blocks only.
fn hold(pool: &block::Pool, least: usize) -> Vec<Unique> {
    let mut blocks = Vec::new();
    let mut len = pool.largest();
    while len >= least {
        while let Ok(block) = pool.alloc(len) {
            blocks.push(block);
        }
        len -= len.div_ceil(16);
    }
    blocks.pop();
    blocks
}

/// A frame whose draft finds no block is refused as `Full`, its body drops, and the
/// stream takes the next frame once the pool has room.
#[test]
fn refuses_a_frame_that_finds_no_block_and_takes_it_after_a_free() {
    let ready = async |test: &Test| {
        let blocks = hold(&test.pool, 1 << 14);
        let clock = test.clock.clone();
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            drop(blocks);
        });
    };
    let stamps: Vec<i64> = (1..=3000).collect();
    let home = read_session(405, ready, move |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send(&mut stream, &stamps, &stamps).await;
        assert_eq!(
            stream.reply().await,
            refused(wire::hub::write::Refusal::Full)
        );
        agent.sleep(START).await;
        send(&mut stream, &stamps, &stamps).await;
        assert_eq!(stream.reply().await, taken(0, 3000));
        assert_eq!(end(&mut stream).await, [stored(2999)]);
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// Fills the pool of `test` at [`START`], and frees it [`START`] later.
async fn fill_for_a_while(test: &Test) {
    let (clock, pool) = (test.clock.clone(), Rc::clone(&test.pool));
    test.tasks.spawn(async move {
        clock.sleep(START).await;
        let blocks = super::fill(&pool);
        clock.sleep(START).await;
        drop(blocks);
    });
}

/// Fills the pool of `test` when a complete reader gets the first frame of `value`, at
/// its commit, and frees it [`START`] later. The run picks the order of the tasks that
/// the commit wakes: in the runs of the tests, the reader runs before the write.
async fn fill_at_commit(test: &Test) {
    let names = ["value"];
    let config = super::unnamed(&names, hub::reader::Mode::Complete);
    let mut reader = test.hub.reader(config).await.expect("opens");
    let (clock, pool) = (test.clock.clone(), Rc::clone(&test.pool));
    test.tasks.spawn(async move {
        let frame = reader.next().await.expect("a frame");
        let blocks = super::fill(&pool);
        clock.sleep(START).await;
        drop((frame, blocks));
    });
}

/// A frame that finds no block for its outcome stops the stream as `BUSY` before the
/// home takes it: the next write takes its first frame at seq 0.
#[test]
fn stops_a_write_whose_frame_finds_no_block_for_its_outcome_as_busy() {
    let home = read_session(424, fill_for_a_while, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        agent.sleep(Span::from_nanos(START.nanos() * 3 / 2)).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(stream.recv().await, reset_with(BUSY));
        agent.sleep(START).await;
        let mut stream = write(&agent).await;
        send(&mut stream, &[2], &[20]).await;
        assert_eq!(stream.reply().await, taken(0, 1));
        assert_eq!(end(&mut stream).await, [stored(0)]);
        finish(&mut agent).await;
    });
    let busy = serve::Error::Pool(block::Error::Exhausted {
        requested: 13,
        available: 64,
    });
    assert_eq!(home.served, [Err(busy), Ok(Got::Ended), Ok(Got::Ended)]);
}

/// A `Stored` that finds no block during the stream is not sent: the next `Stored`
/// covers its frame, and the finish sends no other.
#[test]
fn skips_a_stored_that_finds_no_block_and_covers_it_with_the_next() {
    let home = read_session(433, fill_at_commit, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(stream.reply().await, taken(0, 1));
        agent.sleep(Span::from_nanos(START.nanos() * 3 / 2)).await;
        send(&mut stream, &[2], &[20]).await;
        assert_eq!(stream.reply().await, taken(1, 1));
        assert_eq!(stream.reply().await, stored(1));
        assert_eq!(end(&mut stream).await, []);
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// At the finish, the last `Stored` waits for a block until the pool has one.
#[test]
fn waits_for_a_block_for_the_last_stored_at_the_finish() {
    let home = read_session(426, fill_at_commit, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send(&mut stream, &[1], &[10]).await;
        let sent = agent.node.clock().now();
        assert_eq!(end(&mut stream).await, [taken(0, 1), stored(0)]);
        let waited = agent.node.clock().now() - sent;
        assert!(waited > Span::from_nanos(START.nanos() / 4), "{waited}");
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

/// The wait for a block for the last `Stored` ends when the session closes.
#[test]
fn ends_the_wait_for_the_last_stored_when_the_session_closes() {
    let home = read_session(427, fill_at_commit, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(stream.reply().await, taken(0, 1));
        stream.sender.finish().expect("finishes");
        agent.sleep(Span::from_nanos(START.nanos() / 4)).await;
        agent.session.close(transport::Code(0));
        agent.sleep(START).await;
    });
    let closed = serve::Error::Stream(transport::Error::PeerClosed {
        code: transport::Code(0),
    });
    assert_eq!(home.served, [Err(closed.clone()), Err(closed)]);
}

/// Values of `stamps` that do not compress, so a stored frame is as large as its body.
fn noise(stamps: &[i64]) -> Vec<i64> {
    stamps
        .iter()
        .map(|&stamp| {
            let x = stamp.wrapping_mul(6_364_136_223_846_793_005);
            x ^ (x >> 29)
        })
        .collect()
}

/// The next outcome of a write, past each `Stored`.
async fn outcome(stream: &mut Stream) -> Outcome {
    loop {
        if let Reply::Outcome(outcome) = stream.reply().await {
            return outcome;
        }
    }
}

/// A frame over the body of one write is refused as `Large`, and the stream takes the
/// next frame.
#[test]
fn refuses_a_frame_too_large_for_one_write() {
    let stamps: Vec<i64> = (1..=10_000).collect();
    let values = noise(&stamps);
    let home = read_session(
        406,
        async |_| {},
        move |mut agent| async move {
            agent.admit().await;
            let mut stream = write(&agent).await;
            send(&mut stream, &stamps, &values).await;
            assert_eq!(
                stream.reply().await,
                refused(wire::hub::write::Refusal::Large)
            );
            send(&mut stream, &[1], &[10]).await;
            assert_eq!(stream.reply().await, taken(0, 1));
            assert_eq!(end(&mut stream).await, [stored(0)]);
            finish(&mut agent).await;
        },
    );
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

#[test]
fn stops_a_write_whose_channels_are_unknown_or_on_two_indexes() {
    let home = read_session(
        407,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut unknown = agent.write(&["nope"]).await;
            assert_eq!(unknown.recv().await, reset_with(UNKNOWN));
            let mut many = agent.write(&["value", "value-b"]).await;
            assert_eq!(many.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let unknown = serve::Error::Writer(Box::new(writer::Error::Unknown(name("nope"))));
    assert_eq!(
        home.served,
        [
            Err(unknown.clone()),
            Err(serve::Error::ManyIndexes),
            Ok(Got::Ended)
        ]
    );
    assert_eq!(
        unknown.to_string(),
        "the writer of a write did not open: no channel is named nope"
    );
}

#[test]
fn refuses_a_write_signed_with_another_key_or_as_a_read() {
    let home = read_session(
        408,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let open = Open {
                authority: Authority(1),
                lease: None,
                channels: vec![name("value")],
            };
            for (kind, key) in [(Kind::Write, &OTHER), (Kind::Read, &AGENT)] {
                let mut stream = agent.open().await;
                stream.write(&open, kind, key).await;
                assert_eq!(stream.recv().await, reset_with(REFUSED));
            }
            finish(&mut agent).await;
        },
    );
    let signature = || Err(serve::Error::Access(Refusal::Signature));
    assert_eq!(home.served, [signature(), signature(), Ok(Got::Ended)]);
}

#[test]
fn stops_a_write_whose_lease_is_zero_as_malformed() {
    let home = read_session(
        409,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let open = Open {
                authority: Authority(1),
                lease: Some(Span::ZERO),
                channels: vec![name("value")],
            };
            let mut stream = agent.open().await;
            stream.write(&open, Kind::Write, &AGENT).await;
            assert_eq!(stream.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let lease = home::writer::Error::Lease { span: Span::ZERO };
    let lease = serve::Error::Writer(Box::new(writer::Error::Home(lease)));
    assert_eq!(home.served, [Err(lease), Ok(Got::Ended)]);
}

/// A write holds 16 bytes for each channel of its open and one for its index in the
/// share of its subject until it ends. The open of `value` is 8 bytes, so it fits
/// where its ends do not.
#[test]
fn stops_a_write_whose_ends_pass_the_share_as_busy() {
    let home = read_session(
        410,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let held = agent.unfinished(BODY_BYTES_MAX - 31, &[]).await;
            agent.sleep(Span::MILLISECOND).await;
            let mut busy = agent.write(&["value"]).await;
            assert_eq!(busy.recv().await, reset_with(BUSY));
            drop(held);
            agent.sleep(Span::MILLISECOND).await;
            let mut stream = write(&agent).await;
            let held = agent.unfinished(BODY_BYTES_MAX - 32, &[]).await;
            agent.sleep(Span::MILLISECOND).await;
            send(&mut stream, &[1], &[10]).await;
            assert_eq!(stream.reply().await, taken(0, 1));
            drop(held);
            finish(&mut agent).await;
        },
    );
    let share = serve::Error::Share {
        subject: name(SUBJECT),
        length: 32,
        held: BODY_BYTES_MAX - 31,
    };
    assert_eq!(home.served[0], Err(share.clone()));
    assert_eq!(
        share.to_string(),
        "32 bytes of a body or of the ends of a write do not fit under the share of \
         16777216 bytes of subject ops.agent: its open requests, reads, and writes \
         hold 16777185 bytes"
    );
}

#[test]
fn stops_a_write_whose_channel_is_removed_as_unknown() {
    let ready = async |test: &Test| {
        let (clock, hub) = (test.clock.clone(), test.hub.clone());
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            hub.set_definitions(&without(&["value"]));
        });
    };
    let home = read_session(411, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        agent.sleep(START).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(stream.recv().await, reset_with(UNKNOWN));
        finish(&mut agent).await;
    });
    let removed = serve::Error::Removed(channel::Key::from_u128(2));
    assert_eq!(home.served, [Err(removed), Ok(Got::Ended)]);
}

/// A frame whose body message passes its body, and a frame whose ends name a place
/// that the write does not have, stop the stream as `MALFORMED`.
#[test]
fn stops_a_write_whose_frame_breaks_a_rule_as_malformed() {
    let home = read_session(
        415,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut over = write(&agent).await;
            let (ends, mut body) = frame(&[(0, &[1]), (1, &[10])]);
            body.extend([0; 8]);
            over.frame(live(1, 2), &ends, &body).await;
            assert_eq!(over.recv().await, reset_with(MALFORMED));
            let mut outside = write(&agent).await;
            let (ends, body) = frame(&[(0, &[1]), (2, &[10])]);
            outside.frame(live(1, 2), &ends, &body).await;
            assert_eq!(outside.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let over = wire::hub::Error::Body {
        len: 24,
        remain: 16,
    };
    let outside = types::frame::Error::OutOfRange {
        entry: 2,
        entries: 2,
    };
    assert_eq!(
        home.served,
        [
            Err(serve::Error::Message(over)),
            Err(serve::Error::Frame(outside.clone())),
            Ok(Got::Ended)
        ]
    );
    assert_eq!(
        serve::Error::Frame(outside).to_string(),
        "a frame of a write is not valid: entry 2 is out of range for a key set of 2 \
         entries"
    );
}

/// A program that finishes its stream inside a frame stops it as `MALFORMED`.
#[test]
fn stops_a_write_that_ends_inside_a_frame_as_malformed() {
    let home = read_session(
        412,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut stream = write(&agent).await;
            let mut head = [0; Head::LEN];
            live(1, 2).encode(&mut head);
            stream.send(&head).await;
            stream.sender.finish().expect("finishes");
            assert_eq!(stream.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let cut = wire::hub::Error::Cut {
        received: Head::LEN,
        len: None,
    };
    let cut = serve::Error::Message(cut);
    assert_eq!(home.served, [Err(cut.clone()), Ok(Got::Ended)]);
    assert_eq!(
        cut.to_string(),
        "a hub message is not valid: the stream ended inside a frame: 11 bytes, before \
         the end of its ends"
    );
}

/// A failed sync of the buffer stops a write whose frame waits for its commit as
/// `FAILED`.
#[test]
fn stops_a_write_whose_buffer_failed_as_failed() {
    let ready = async |test: &Test| {
        let (clock, node) = (test.clock.clone(), test.node.clone());
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            node.fail_file(FilePath::new(RING), Operation::Sync);
        });
    };
    let home = read_session(413, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        agent.sleep(START).await;
        send(&mut stream, &[1], &[10]).await;
        assert_eq!(stream.reply().await, taken(0, 1));
        assert_eq!(stream.recv().await, reset_with(FAILED));
        finish(&mut agent).await;
    });
    let failed = serve::Error::Buffer(env::files::Error::Io {
        path: PathBuf::from(RING),
        operation: Operation::Sync,
        code: 5,
    });
    assert_eq!(home.served, [Err(failed), Ok(Got::Ended)]);
}

/// A failed sync of the buffer after the program finishes its stream stops the write
/// as `FAILED`, with no `Stored` for the frame that waits for that commit.
#[test]
fn stops_a_write_whose_last_commit_failed_as_failed() {
    let ready = async |test: &Test| {
        let (clock, node) = (test.clock.clone(), test.node.clone());
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            node.fail_file(FilePath::new(RING), Operation::Sync);
        });
    };
    let home = read_session(422, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = write(&agent).await;
        agent.sleep(START).await;
        send(&mut stream, &[1], &[10]).await;
        stream.sender.finish().expect("finishes");
        assert_eq!(stream.reply().await, taken(0, 1));
        assert_eq!(stream.recv().await, reset_with(FAILED));
        finish(&mut agent).await;
    });
    let failed = serve::Error::Buffer(env::files::Error::Io {
        path: PathBuf::from(RING),
        operation: Operation::Sync,
        code: 5,
    });
    assert_eq!(home.served, [Err(failed), Ok(Got::Ended)]);
}

/// The home of `TIME` is another node and that of `TIME_B` is this node: it serves
/// one session of a program and records what each stream gave into `kept`.
async fn split_home(node: sim::node::Node, tasks: env::tasks::Tasks, kept: Kept) {
    let transport = Rc::new(transport(&node, &tasks, &own_pool(), HOME, 1 << 16));
    let region =
        region::open(&node, &tasks, Rc::clone(&transport), Vec::new(), channels())
            .await;
    let layout = buffer::Layout::new(AREA, BODY_MAX).expect("a ring");
    let mut test =
        Test::new(node.clone(), tasks.clone(), layout, POOL, Some(region)).await;
    test.sync().await;
    test.set_home(TIME, region::OTHER).await;
    test.set_home(TIME_B, NODE).await;
    test.hub.set_rules(rules());
    let session = transport.accept().await.expect("a session");
    let link = test.hub.link(session.clone());
    let accepted = Rc::new(Cell::new(0));
    tasks.spawn(serve_each(
        &session,
        &link,
        &tasks,
        &node.clock(),
        &kept,
        &accepted,
    ));
    session.closed().await;
    recorded(&node, &kept, accepted.get()).await;
}

/// A write on two indexes stops as `MALFORMED` before it waits for a home, also when
/// the home of one index is another node.
#[test]
fn stops_a_write_on_two_indexes_before_it_waits_for_a_home() {
    let served = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&served);
    let home = move |node, tasks| split_home(node, tasks, kept);
    run_program(423, home, |node, tasks, at| {
        as_agent(node, tasks, at, |mut agent| async move {
            agent.admit().await;
            let mut stream = agent.write(&["value", "value-b"]).await;
            assert_eq!(stream.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        })
    });
    let served = std::mem::take(&mut *served.lock().expect("not poisoned"));
    assert_eq!(served, [Err(serve::Error::ManyIndexes), Ok(Got::Ended)]);
}

/// A node serves a write only of an index whose home it is: a write whose home is
/// another node stops with `NOT_HOME`, and takes no control there.
#[test]
fn stops_a_write_whose_home_is_another_node_as_not_home() {
    let served = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&served);
    let home = move |node, tasks| split_home(node, tasks, kept);
    run_program(414, home, |node, tasks, at| {
        as_agent(node, tasks, at, |mut agent| async move {
            agent.admit().await;
            let mut stream = agent.write(&["value"]).await;
            assert_eq!(stream.recv().await, reset_with(NOT_HOME));
            finish(&mut agent).await;
        })
    });
    let remote = writer::Error::Remote {
        home: region::OTHER,
    };
    let served = std::mem::take(&mut *served.lock().expect("not poisoned"));
    assert_eq!(
        served,
        [Err(serve::Error::Writer(Box::new(remote))), Ok(Got::Ended)]
    );
}
