//! The read streams of a client session that `Link::serve` serves, over a real
//! transport from a program to a simulated node.

use access::proof::{Error as Refusal, Kind};
use hub::{reader, serve};
use transport::Class;
use types::channel;
use types::sample::Type;
use types::time::Span;
use wire::header::MALFORMED;
use wire::hub::client::{
    self, BODY_BYTES_MAX, FromNode, Mode, Open, Place, Places, REFUSED,
};
use wire::hub::{BUSY, FromHome, UNKNOWN};

use super::link::{Home, OTHER, QUIET, as_agent, serve_session_on};
use super::serve::{decoded, got, next, places as placed};
use super::{
    COMMIT, DataType, I64, POOL, STAMP, Test, channels, without, write, write_wide,
};
use crate::agent::{AGENT, Agent, SUBJECT, Stream, name, reset_with};
use crate::sessions::{Got, rules};

/// How long after the home accepts the session its first write or change comes, so
/// the program has opened its read by then.
const START: Span = Span::from_nanos(100_000_000);

/// Runs one client session on a synced [`Test`] hub, as `link::session` does, where
/// `ready` gets the hub before the home serves the session's streams.
fn read_session<P>(
    seed: u64,
    ready: impl AsyncFnOnce(&Test) + Send + 'static,
    program: impl FnOnce(Agent) -> P + Send + 'static,
) -> Home
where
    P: Future<Output = ()> + 'static,
{
    serve_session_on(
        seed,
        sim::link::Config::default(),
        true,
        POOL,
        Some(rules()),
        ready,
        move |node, tasks, at| as_agent(node, tasks, at, program),
    )
}

/// Writes each of `values` to `value` in a frame of its own, [`COMMIT`] apart, from
/// [`START`] on.
async fn writes(test: &Test, values: &'static [i64]) {
    let mut writer = test.writer("a", &["value"]).await;
    let (clock, now) = (test.clock.clone(), test.now());
    test.tasks.spawn(async move {
        clock.sleep(START).await;
        for (stamp, &value) in (now..).zip(values) {
            write(&mut writer, &[stamp], &[value]);
            clock.sleep(Span::from_nanos(2 * COMMIT.nanos())).await;
        }
    });
}

/// Writes `frames` wide frames to `value` from [`START`] on, in runs of ten that are
/// 2 × [`COMMIT`] apart.
async fn writes_wide(test: &Test, frames: i64) {
    let mut writer = test.writer("a", &["value"]).await;
    let (clock, now) = (test.clock.clone(), test.now());
    test.tasks.spawn(async move {
        clock.sleep(START).await;
        for n in 0..frames {
            write_wide(&mut writer, now, n);
            if n % 10 == 9 {
                clock.sleep(Span::from_nanos(2 * COMMIT.nanos())).await;
            }
        }
    });
}

fn open(mode: Mode, select: &[&str]) -> Open {
    Open {
        mode,
        select: types::name::Selector::new(select.iter().copied()).expect("a selector"),
    }
}

fn place(key: u128, data_type: Type, channel: &str) -> Place {
    Place {
        key: channel::Key::from_u128(key),
        data_type,
        name: name(channel),
    }
}

/// Reads the start of a read in `mode`: the places, their run, and `Opened`. Gives
/// the places, the run, and the decoder of the frames.
async fn opened(
    stream: &mut Stream,
    mode: Mode,
) -> (Places, Vec<Place>, wire::hub::Reader) {
    let mut start = client::Reader::new(mode);
    let message = next(&mut stream.receiver).await;
    let Ok(FromNode::Places(places)) = start.decode(&message) else {
        panic!("the places come first");
    };
    let mut run = Vec::new();
    let mut reader = loop {
        let message = next(&mut stream.receiver).await;
        let Ok(FromNode::Run { places, home }) = start.decode(&message) else {
            panic!("a message of the run of places");
        };
        run.extend(places);
        if let Some(home) = home {
            break home;
        }
    };
    let message = next(&mut stream.receiver).await;
    assert!(matches!(reader.decode(&message), Ok(FromHome::Opened)));
    (places, run, reader)
}

/// Finishes the hello stream and waits, so the hello stream ends as `Ended`.
async fn finish(agent: &mut Agent) {
    agent.hello.sender.finish().expect("finishes");
    agent.sleep(QUIET).await;
}

/// The places of a read are the channels that its selector matches, in name order,
/// then its index when the selector does not match it.
#[test]
fn sends_the_places_and_each_frame_of_a_complete_read() {
    let ready = async |test: &Test| writes(test, &[10, 20]).await;
    let home = read_session(300, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Complete, &["value"]).await;
        let (places, run, mut reader) = opened(&mut stream, Mode::Complete).await;
        assert_eq!(places, Places { count: 2, index: 1 });
        assert_eq!(run, [place(2, I64, "value"), place(1, STAMP, "time")]);
        for value in [10, 20] {
            let got = got(&mut stream.receiver, &mut reader)
                .await
                .expect("a frame");
            assert_eq!(placed(&got), [0, 1]);
            assert_eq!(decoded(&got, &[I64, STAMP])[0], [value]);
        }
        stream.sender.finish().expect("finishes");
        assert_eq!(stream.recv().await, Ok(None));
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

#[test]
fn sends_a_latest_read_the_index_once_when_its_selector_matches_it() {
    let ready = async |test: &Test| writes(test, &[10]).await;
    let home = read_session(301, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Latest, &["value", "time"]).await;
        let (places, run, mut reader) = opened(&mut stream, Mode::Latest).await;
        assert_eq!(places, Places { count: 2, index: 0 });
        assert_eq!(run, [place(1, STAMP, "time"), place(2, I64, "value")]);
        let got = got(&mut stream.receiver, &mut reader)
            .await
            .expect("a frame");
        assert_eq!(placed(&got), [0, 1]);
        assert_eq!(decoded(&got, &[STAMP, I64])[1], [10]);
        stream.sender.finish().expect("finishes");
        assert_eq!(stream.recv().await, Ok(None));
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}

#[test]
fn refuses_a_read_signed_with_another_key_or_as_a_request() {
    let home = read_session(
        302,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            for (kind, key) in [(Kind::Read, &OTHER), (Kind::Request, &AGENT)] {
                let mut stream = agent.open_as(Class::Complete).await;
                stream
                    .read(&open(Mode::Complete, &["value"]), kind, key)
                    .await;
                assert_eq!(stream.recv().await, reset_with(REFUSED));
            }
            finish(&mut agent).await;
        },
    );
    let signature = || Err(serve::Error::Access(Refusal::Signature));
    assert_eq!(home.served, [signature(), signature(), Ok(Got::Ended)]);
}

#[test]
fn stops_a_read_whose_selector_matches_no_channel_or_two_indexes() {
    let home = read_session(
        303,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut empty = agent.read(Mode::Complete, &["nope"]).await;
            assert_eq!(empty.recv().await, reset_with(UNKNOWN));
            let mut many = agent.read(Mode::Latest, &["value", "value-b"]).await;
            assert_eq!(many.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let empty = serve::Error::Reader(Box::new(reader::Error::Empty));
    let many = reader::Error::ManyIndexes {
        first: name("value"),
        other: name("value-b"),
    };
    assert_eq!(
        home.served,
        [
            Err(empty.clone()),
            Err(serve::Error::Reader(Box::new(many))),
            Ok(Got::Ended)
        ]
    );
    assert_eq!(
        empty.to_string(),
        "the reader of a read did not open: the selector matches no channel"
    );
}

/// A latest read takes a `Latest` stream, and a complete read a `Complete` stream.
#[test]
fn stops_a_read_on_a_stream_of_the_other_class_as_malformed() {
    let home = read_session(
        304,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            for (class, mode) in [
                (Class::Complete, Mode::Latest),
                (Class::Latest, Mode::Complete),
            ] {
                let mut stream = agent.open_as(class).await;
                stream
                    .read(&open(mode, &["value"]), Kind::Read, &AGENT)
                    .await;
                assert_eq!(stream.recv().await, reset_with(MALFORMED));
            }
            finish(&mut agent).await;
        },
    );
    assert_eq!(
        home.served,
        [
            Err(serve::Error::Class(Class::Complete)),
            Err(serve::Error::Class(Class::Latest)),
            Ok(Got::Ended)
        ]
    );
}

/// A read reserves its open under the share of its subject, not the request turn of
/// its link, and frees it once the open is verified. The open of `value` is 8 bytes,
/// and that of `time` and `value` 14.
#[test]
fn opens_reads_beside_a_request_and_stops_one_over_the_share_as_busy() {
    let home = read_session(
        305,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let _held = agent.unfinished(BODY_BYTES_MAX - 8, &[]).await;
            agent.sleep(Span::MILLISECOND).await;
            let mut busy = agent.read(Mode::Complete, &["time", "value"]).await;
            assert_eq!(busy.recv().await, reset_with(BUSY));
            for _ in 0..2 {
                let mut stream = agent.read(Mode::Complete, &["value"]).await;
                let (places, _, _) = opened(&mut stream, Mode::Complete).await;
                assert_eq!(places, Places { count: 2, index: 1 });
            }
        },
    );
    let share = serve::Error::Share {
        subject: name(SUBJECT),
        length: 14,
        held: BODY_BYTES_MAX - 8,
    };
    assert_eq!(home.served[0], Err(share.clone()));
    assert_eq!(
        share.to_string(),
        "a body of 14 bytes does not fit under the share of 16777216 bytes of subject \
         ops.agent: its open requests and reads hold 16777208 bytes"
    );
}

#[test]
fn stops_a_read_whose_channel_is_removed_as_unknown() {
    let ready = async |test: &Test| {
        let (clock, hub) = (test.clock.clone(), test.hub.clone());
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            hub.set_definitions(&without(&["value"]));
        });
    };
    let home = read_session(306, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Complete, &["value"]).await;
        opened(&mut stream, Mode::Complete).await;
        assert_eq!(stream.recv().await, reset_with(UNKNOWN));
        finish(&mut agent).await;
    });
    let removed = reader::Ended::Removed(channel::Key::from_u128(2));
    let removed = serve::Error::Ended(Box::new(removed));
    assert_eq!(home.served, [Err(removed.clone()), Ok(Got::Ended)]);
    assert_eq!(
        removed.to_string(),
        "the reader of a read ended: channel 00000000-0000-0000-0000-000000000002 was \
         removed: open a new reader"
    );
}

#[test]
fn stops_a_read_that_gets_a_message_after_its_open_as_malformed() {
    let home = read_session(
        307,
        async |_| {},
        |mut agent| async move {
            agent.admit().await;
            let mut stream = agent.read(Mode::Latest, &["value"]).await;
            opened(&mut stream, Mode::Latest).await;
            stream.send(b"x").await;
            assert_eq!(stream.recv().await, reset_with(MALFORMED));
            finish(&mut agent).await;
        },
    );
    let trailing = serve::Error::Client(client::Error::Trailing);
    assert_eq!(home.served, [Err(trailing), Ok(Got::Ended)]);
}

/// A message after the open stops a read while its frames still come.
#[test]
fn stops_a_read_with_frames_to_send_at_a_message_after_its_open() {
    const FRAMES: i64 = 400;
    let ready = async |test: &Test| writes_wide(test, FRAMES).await;
    let home = read_session(310, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Complete, &["value"]).await;
        let (_, _, mut reader) = opened(&mut stream, Mode::Complete).await;
        agent.sleep(Span::SECOND).await;
        stream.send(b"x").await;
        let mut frames = 0;
        let end = loop {
            match stream.recv().await {
                Ok(Some(message)) => {
                    if let Ok(FromHome::Head(_)) = reader.decode(&message) {
                        frames += 1;
                    }
                }
                end => break end,
            }
        };
        assert_eq!(end, reset_with(MALFORMED));
        assert!(frames > 0, "frames came before the stop");
        finish(&mut agent).await;
    });
    let trailing = serve::Error::Client(client::Error::Trailing);
    assert_eq!(home.served, [Err(trailing), Ok(Got::Ended)]);
}

/// The channels of a read are removed while its run of places waits for room in the
/// program's window.
#[test]
fn stops_a_read_whose_channel_is_removed_while_its_places_wait_as_unknown() {
    const MANY: u128 = 10_000;
    let ready = async |test: &Test| {
        let long = "x".repeat(240);
        let names: Vec<String> = (0..MANY).map(|n| format!("w.{long}.{n}")).collect();
        test.define(names.iter().enumerate().map(|(n, channel)| {
            let key = 100 + u128::try_from(n).expect("fits");
            (key, channel.as_str(), DataType::Sample(I64), 1)
        }));
        let (clock, hub) = (test.clock.clone(), test.hub.clone());
        test.tasks.spawn(async move {
            clock.sleep(START).await;
            hub.set_definitions(&channels());
        });
    };
    let home = read_session(309, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Complete, &["w.**"]).await;
        agent.sleep(Span::from_nanos(2 * START.nanos())).await;
        let end = loop {
            match stream.recv().await {
                Ok(Some(_)) => {}
                end => break end,
            }
        };
        assert_eq!(end, reset_with(UNKNOWN));
        finish(&mut agent).await;
    });
    let removed = reader::Ended::Removed(channel::Key::from_u128(100));
    assert_eq!(
        home.served,
        [Err(serve::Error::Ended(Box::new(removed))), Ok(Got::Ended)]
    );
}

/// A complete read whose program does not take its frames misses one, so the node
/// sends `Behind` after the frames before it, and finishes the stream.
#[test]
fn sends_behind_and_finishes_a_complete_read_that_misses_a_frame() {
    const FRAMES: i64 = 400;
    let ready = async |test: &Test| writes_wide(test, FRAMES).await;
    let home = read_session(308, ready, |mut agent| async move {
        agent.admit().await;
        let mut stream = agent.read(Mode::Complete, &["value"]).await;
        let (_, _, mut reader) = opened(&mut stream, Mode::Complete).await;
        agent.sleep(Span::SECOND).await;
        let mut frames = 0;
        while got(&mut stream.receiver, &mut reader).await.is_some() {
            frames += 1;
        }
        assert!(
            frames > 0 && frames < FRAMES,
            "{frames} frames before behind"
        );
        finish(&mut agent).await;
    });
    assert_eq!(home.served, [Ok(Got::Ended), Ok(Got::Ended)]);
}
