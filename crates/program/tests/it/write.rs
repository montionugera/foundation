//! The writes of a session against the serve of a real hub.

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

use env::clock::Clock;
use env::tasks::Tasks;
use program::write::{Config, Outcome, Refusal as Refused, Time};
use program::{Error, Reader, Session, Writer};
use spec::channel::{Channel, Data, Kind};
use spec::data_type::DataType;
use spec::definition::Definition;
use transport::Code;
use transport::stream::Incoming;
use types::authority::Authority;
use types::channel;
use types::frame::{Draft, Form, Frame, Path, Range};
use types::name::Selector;
use types::sample::{Scalar, Type};
use wire::header::MALFORMED;
use wire::hub::client::write::Write;
use wire::hub::client::{Mode, OPEN_BYTES_MAX, Place, Refusal};

use crate::agent::{AGENT, name};
use crate::connect::{ample, connect, connect_with};
use crate::hand::{accept_stream, by_hand, send, start};
use crate::node::env;
use crate::session::{admit, raw, within};
use crate::sessions::{self, Home, QUIET, rules};

const STAMP: Type = Type::Scalar(Scalar::Stamp);
const I64: Type = Type::Scalar(Scalar::I64);

fn key(key: u128) -> channel::Key {
    channel::Key::from_u128(key)
}

fn place(at: u128, data_type: Type, channel: &str) -> Place {
    Place {
        key: key(at),
        data_type,
        name: name(channel),
    }
}

/// `time` with key 1, and `value` and `extra` with keys 2 and 3, `i64` channels on
/// `time`.
fn definitions() -> BTreeMap<types::name::Name, Definition> {
    let time = Channel {
        key: key(1),
        kind: Kind::Index {
            error: None,
            control: None,
        },
    };
    let data = |at| Channel {
        key: key(at),
        kind: Kind::Data(
            Data::new(time.key, None, DataType::Sample(I64), None).expect("no unit"),
        ),
    };
    BTreeMap::from([
        (name("time"), Definition::Channel(time.clone())),
        (name("value"), Definition::Channel(data(2))),
        (name("extra"), Definition::Channel(data(3))),
    ])
}

/// Runs `program` with a session of `SUBJECT` on a hub with the channels of
/// [`definitions`]. The program gets the clock of its node. Gives what the home saw.
fn on_hub<P>(
    seed: u64,
    program: impl FnOnce(Session, Clock) -> P + Send + 'static,
) -> Home
where
    P: Future<Output = ()> + 'static,
{
    let open = async |node: &sim::node::Node, tasks: &Tasks| {
        let (hub, _) = hub::testing::open(env(node, tasks.clone())).await;
        hub.set_definitions(&definitions());
        (hub, ())
    };
    sessions::serve_session(
        seed,
        sim::link::Config::default(),
        Some(rules()),
        open,
        async |()| {},
        |node, tasks, at| async move {
            let session = connect(&node, tasks, at, AGENT).await.expect("connects");
            program(session, node.clock()).await;
            node.clock().sleep(QUIET).await;
        },
    )
}

fn config(channels: &[&str], authority: u8) -> Config {
    Config {
        authority: Authority(authority),
        lease: None,
        channels: channels.iter().map(|channel| name(channel)).collect(),
    }
}

async fn reader(session: &Session, channels: &[&str]) -> Reader {
    let select = Selector::new(channels.iter().copied()).expect("a selector");
    session.reader(select, Mode::Complete).await.expect("opens")
}

/// The entry of the place of channel `at` of `places`.
fn entry(places: &[Place], at: u128) -> usize {
    places
        .iter()
        .position(|place| place.key == key(at))
        .expect("a place")
}

/// Fills the draft of `writer`: a raw frame with `count` samples a series: `stamps` on `time`, then
/// each of `values` on the channel of its key. A sample `n` of a series is its first
/// value plus `n`.
fn frame<'a>(
    writer: &'a mut Writer,
    count: usize,
    stamp: i64,
    values: &[(u128, i64)],
) -> &'a mut Draft {
    let mut series: Vec<(usize, i64)> = [(1, stamp)]
        .iter()
        .chain(values)
        .map(|&(at, first)| (entry(writer.places(), at), first))
        .collect();
    series.sort_unstable();
    let lens: Vec<(usize, usize)> =
        series.iter().map(|&(at, _)| (at, 8 * count)).collect();
    let draft = writer.draft(Form::Raw, &lens).expect("a frame");
    for (at, first) in series {
        let bytes = draft.series_mut(at).expect("present");
        let (samples, _) = bytes.as_chunks_mut::<8>();
        for (n, sample) in samples.iter_mut().enumerate() {
            let n = i64::try_from(n).expect("fits");
            *sample = (first + n).to_le_bytes();
        }
    }
    draft.set_count(0, u32::try_from(count).expect("fits"));
    draft
}

/// Each series of `frame`, a frame of `reader`, as its key and its samples.
fn samples(reader: &Reader, frame: &Frame) -> Vec<(channel::Key, Vec<i64>)> {
    let count = frame.range(0).expect("the index group").count;
    let count = usize::try_from(count).expect("fits");
    frame
        .iter()
        .map(|(entry, bytes)| {
            let mut out = Vec::new();
            let place = &reader.places()[entry];
            codec::decode(place.data_type, count, bytes, &mut out).expect("decodes");
            let (chunks, _) = out.as_chunks::<8>();
            (
                place.key,
                chunks
                    .iter()
                    .map(|chunk| i64::from_le_bytes(*chunk))
                    .collect(),
            )
        })
        .collect()
}

/// The stamp of the one sample of `frame`, a frame of `reader`.
fn stamp_of(reader: &Reader, frame: &Frame) -> i64 {
    let samples = samples(reader, frame);
    let (_, stamps) = samples
        .iter()
        .find(|(at, _)| *at == key(1))
        .expect("the index");
    stamps[0]
}

fn taken(seq: u64, count: u32) -> Outcome {
    Outcome::Taken {
        range: Range { seq, count },
    }
}

/// The node stamps a frame of `Time::Node`, a frame of `Time::Frame` holds its own
/// stamps, and a reader on the home gets both in order.
#[test]
fn writes_each_frame_to_a_reader_on_the_home_in_order() {
    on_hub(700, |session, _| async move {
        let mut reader = reader(&session, &["value"]).await;
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        assert_eq!(
            writer.places(),
            [place(1, STAMP, "time"), place(2, I64, "value")]
        );
        frame(&mut writer, 1, 0, &[(2, 10)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        let first = reader.next().await.expect("a frame");
        let stamp = stamp_of(&reader, &first);
        assert_eq!(samples(&reader, &first)[0], (key(2), vec![10]));
        frame(&mut writer, 2, stamp + 5, &[(2, 20)]);
        let outcome = writer.write(Time::Frame(Path::Live)).await;
        assert_eq!(outcome, Ok(taken(1, 2)));
        let second = reader.next().await.expect("a frame");
        assert_eq!(
            samples(&reader, &second),
            [(key(2), vec![20, 21]), (key(1), vec![stamp + 5, stamp + 6])]
        );
    });
}

/// A frame whose series fill fewer places than the writer has puts each series at its
/// own place.
#[test]
fn writes_a_frame_that_fills_fewer_places_than_the_writer_has() {
    on_hub(701, |session, _| async move {
        let mut reader = reader(&session, &["extra", "value"]).await;
        let mut writer = session
            .writer(config(&["value", "extra"], 1))
            .await
            .expect("opens");
        frame(&mut writer, 1, 0, &[(3, 30)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        let got = reader.next().await.expect("a frame");
        let got = samples(&reader, &got);
        let keys: Vec<channel::Key> = got.iter().map(|(at, _)| *at).collect();
        assert!(keys.contains(&key(1)) && keys.contains(&key(3)), "{got:?}");
        assert!(!keys.contains(&key(2)), "{got:?}");
        assert!(got.contains(&(key(3), vec![30])), "{got:?}");
    });
}

/// A frame of a body over the most bytes of one message goes in more than one, and
/// the reader gets it whole.
#[test]
fn writes_a_body_over_one_message_whole() {
    const COUNT: usize = 20_000;
    on_hub(702, |session, _| async move {
        let mut reader = reader(&session, &["value"]).await;
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, COUNT, 1_000, &[(2, 7)]);
        let count = u32::try_from(COUNT).expect("fits");
        let outcome = writer.write(Time::Frame(Path::Live)).await;
        assert_eq!(outcome, Ok(taken(0, count)));
        let got = reader.next().await.expect("a frame");
        let got = samples(&reader, &got);
        let values: Vec<i64> = (7..).take(COUNT).collect();
        let stamps: Vec<i64> = (1_000..).take(COUNT).collect();
        assert_eq!(got, [(key(2), values), (key(1), stamps)]);
    });
}

/// A frame of `Time::Frame` whose stamp is not after the last stamp that the node
/// wrote is refused, and the writer goes on.
#[test]
fn refuses_a_frame_whose_stamp_is_not_after_the_last_stamp_of_the_node() {
    on_hub(703, |session, _| async move {
        let mut reader = reader(&session, &["value"]).await;
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 10)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        let first = reader.next().await.expect("a frame");
        let stamp = stamp_of(&reader, &first);
        frame(&mut writer, 1, stamp, &[(2, 11)]);
        let outcome = writer.write(Time::Frame(Path::Live)).await;
        assert_eq!(outcome, Ok(Outcome::Refused(Refused::Order)));
        frame(&mut writer, 1, 0, &[(2, 12)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(1, 1)));
        let next = reader.next().await.expect("a frame");
        let later = stamp_of(&reader, &next);
        assert!(later > stamp, "{later} after {stamp}");
    });
}

/// A second writer on a channel whose control the first holds gets `Waiting`.
#[test]
fn gives_waiting_to_a_second_writer_while_the_first_holds_control() {
    on_hub(704, |session, _| async move {
        let mut first = session.writer(config(&["value"], 2)).await.expect("opens");
        let mut second = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut first, 1, 0, &[(2, 1)]);
        assert_eq!(first.write(Time::Node).await, Ok(taken(0, 1)));
        frame(&mut second, 1, 0, &[(2, 2)]);
        let outcome = second.write(Time::Node).await;
        assert_eq!(outcome, Ok(Outcome::Refused(Refused::Waiting)));
    });
}

/// `stored` gives at once with no taken frame, and waits for the commit of a taken
/// frame.
#[test]
fn gives_stored_once_the_commit_covers_the_last_taken_frame() {
    on_hub(705, |session, clock| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        {
            let mut stored = pin!(writer.stored());
            let now = poll_fn(|cx| Poll::Ready(stored.as_mut().poll(cx)));
            assert_eq!(now.await, Poll::Ready(Ok(())));
        }
        frame(&mut writer, 1, 0, &[(2, 1)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        let before = clock.now();
        assert_eq!(writer.stored().await, Ok(()));
        assert!(clock.now() > before, "stored waits for the commit");
        let mut stored = pin!(writer.stored());
        let now = poll_fn(|cx| Poll::Ready(stored.as_mut().poll(cx)));
        assert_eq!(now.await, Poll::Ready(Ok(())));
    });
}

/// A frame that the node cannot take as it is gives `Unsent` with nothing sent, and
/// the writer goes on.
#[test]
fn gives_unsent_for_a_frame_of_no_sample_or_a_bad_frame_to_stamp() {
    on_hub(706, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 1)]).set_count(0, 0);
        let got = writer.write(Time::Frame(Path::Live)).await;
        assert_eq!(
            got,
            Err(Error::Unsent {
                count: 0,
                form: Form::Raw
            })
        );
        assert_eq!(
            got.unwrap_err().to_string(),
            "the frame holds no sample, so it was not sent"
        );
        frame(&mut writer, 2, 0, &[(2, 1)]);
        let got = writer.write(Time::Node).await;
        assert_eq!(
            got,
            Err(Error::Unsent {
                count: 2,
                form: Form::Raw
            })
        );
        assert_eq!(
            got.unwrap_err().to_string(),
            "a frame that the node stamps holds 1 sample in the raw form, not 2 in the \
             Raw form, so it was not sent"
        );
        let places = writer.places().len();
        let series: Vec<(usize, usize)> = (0..places).map(|at| (at, 8)).collect();
        let encoded = writer.draft(Form::Encoded, &series).expect("a frame");
        encoded.set_count(0, 1);
        let got = writer.write(Time::Node).await;
        assert_eq!(
            got,
            Err(Error::Unsent {
                count: 1,
                form: Form::Encoded
            })
        );
        frame(&mut writer, 1, 0, &[(2, 1)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
    });
}

/// A writer of no channel, or of an open over the cap, sends nothing.
#[test]
fn refuses_a_writer_of_no_channel_or_over_the_cap_with_nothing_sent() {
    let names: Vec<String> = (0..300)
        .map(|n| format!("n{n:03}{}", "x".repeat(240)))
        .collect();
    let channels: Vec<_> = names.iter().map(|n| name(n)).collect();
    let open = wire::hub::client::write::Open {
        authority: Authority(1),
        lease: None,
        channels: channels.clone(),
    };
    let length = open.encoded_len();
    assert!(u64::try_from(length).expect("fits") > OPEN_BYTES_MAX);
    let home = on_hub(707, move |session, _| async move {
        let got = session.writer(config(&[], 1)).await;
        let error = got.map(drop).unwrap_err();
        assert_eq!(error, Error::Unnamed);
        assert_eq!(error.to_string(), "the writer names no channel");
        let over = Config {
            authority: Authority(1),
            lease: None,
            channels,
        };
        let got = session.writer(over).await;
        assert_eq!(got.map(drop), Err(Error::Open { length }));
    });
    assert_eq!(home.served.len(), 1, "only the hello: {:?}", home.served);
}

/// A write that the node refuses, as it refuses one signed with another key, gives
/// [`Refusal::Refused`].
#[test]
fn gives_the_refusal_of_a_write_that_the_node_refuses() {
    by_hand(
        708,
        |session, node| async move {
            let mut write = accept_stream(&session, |first| {
                Write::decode(first).expect("a write").body()
            })
            .await;
            let code = Code(Refusal::Refused.code());
            write.receiver.stop(code);
            write.sender.take().expect("two-way").reset(code);
            node.clock().sleep(QUIET).await;
        },
        |session, node| {
            Box::pin(async move {
                let got = session.writer(config(&["value"], 1)).await;
                let error = got.map(drop).unwrap_err();
                assert_eq!(error, Error::Refused(Refusal::Refused));
                assert_eq!(error.to_string(), Refusal::Refused.to_string());
                node.clock().sleep(QUIET).await;
            })
        },
    );
}

/// A call dropped after it sent its frame loses its outcome, and the next call gives
/// its own.
#[test]
fn gives_the_next_outcome_after_a_call_that_dropped_after_its_send() {
    on_hub(709, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        {
            frame(&mut writer, 1, 0, &[(2, 1)]);
            let mut write = pin!(writer.write(Time::Node));
            let waits = poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx).is_pending()));
            assert!(waits.await, "the write waits for its outcome");
        }
        frame(&mut writer, 1, 0, &[(2, 2)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(1, 1)));
    });
}

/// A `stored` after a call that dropped after its send waits for the commit of that
/// frame.
#[test]
fn waits_in_stored_for_the_frame_of_a_call_that_dropped_after_its_send() {
    on_hub(717, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        {
            frame(&mut writer, 1, 0, &[(2, 1)]);
            let mut write = pin!(writer.write(Time::Node));
            let waits = poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx).is_pending()));
            assert!(waits.await, "the write waits for its outcome");
        }
        {
            let mut stored = pin!(writer.stored());
            let waits =
                poll_fn(|cx| Poll::Ready(stored.as_mut().poll(cx).is_pending()));
            assert!(waits.await, "stored waits for the outcome and the commit");
            assert_eq!(stored.await, Ok(()));
        }
        frame(&mut writer, 1, 0, &[(2, 2)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(1, 1)));
    });
}

/// An open of exactly the cap reaches the node, which finds no channel.
#[test]
fn sends_an_open_at_the_cap() {
    let open = |names: &[String]| wire::hub::client::write::Open {
        authority: Authority(1),
        lease: None,
        channels: names.iter().map(|n| name(n)).collect(),
    };
    let mut names: Vec<String> = (0..256)
        .map(|n| format!("n{n:03}{}", "x".repeat(251)))
        .collect();
    let over =
        open(&names).encoded_len() - usize::try_from(OPEN_BYTES_MAX).expect("fits");
    names[0].truncate(255 - over);
    let open = open(&names);
    assert_eq!(u64::try_from(open.encoded_len()), Ok(OPEN_BYTES_MAX));
    on_hub(718, move |session, _| async move {
        let config = Config {
            authority: open.authority,
            lease: open.lease,
            channels: open.channels,
        };
        let got = session.writer(config).await.map(drop);
        assert_eq!(got, Err(Error::Refused(Refusal::Unknown)));
    });
}

/// A frame whose ends are over the most bytes of one message sends them in more than
/// one, each of whole ends, and the node takes them in order.
#[test]
fn sends_ends_over_one_message_as_whole_ends() {
    const SERIES: usize = 1 << 15;
    let places: Vec<Place> = [place(1, STAMP, "time")]
        .into_iter()
        .chain(
            (2..)
                .take(SERIES - 1)
                .map(|at| place(at, I64, &format!("c{at:05}"))),
        )
        .collect();
    by_hand(
        719,
        move |session, node| async move {
            let mut write = accept_stream(&session, |first| {
                Write::decode(first).expect("a write").body()
            })
            .await;
            start(&mut write, &places, 0, places.len()).await;
            let mut next = async || {
                let message = write.receiver.recv().await.expect("receives");
                message.expect("a message of the frame").to_vec()
            };
            next().await;
            let mut ends = Vec::new();
            let mut messages = 0;
            while ends.len() < SERIES * 8 {
                let message = next().await;
                assert_eq!(message.len() % 8, 0, "whole ends");
                ends.extend_from_slice(&message);
                messages += 1;
            }
            assert!(messages > 1, "the ends go in {messages} messages");
            let want: Vec<u8> = (1..=SERIES)
                .flat_map(|at| {
                    let at = u32::try_from(at).expect("fits");
                    [(at - 1).to_le_bytes(), (8 * at).to_le_bytes()].concat()
                })
                .collect();
            assert_eq!(ends, want);
            let mut body = 0;
            while body < SERIES * 8 {
                body += next().await.len();
            }
            let reply = wire::hub::write::Reply::Outcome(taken(0, 1));
            let mut bytes = vec![0; reply.encoded_len()];
            reply.encode(&mut bytes);
            send(write.sender.as_mut().expect("two-way"), &bytes).await;
            node.clock().sleep(QUIET).await;
        },
        |session, node| {
            Box::pin(async move {
                let mut writer =
                    session.writer(config(&["c00002"], 1)).await.expect("opens");
                let series: Vec<(usize, usize)> =
                    (0..SERIES).map(|at| (at, 8)).collect();
                let draft = writer.draft(Form::Raw, &series).expect("a frame");
                draft.set_count(0, 1);
                let outcome = writer.write(Time::Frame(Path::Live)).await;
                assert_eq!(outcome, Ok(taken(0, 1)));
                node.clock().sleep(QUIET).await;
            })
        },
    );
}

/// A call dropped while it sent its frame ends the writer, and the home gets no part
/// of the cut frame or of the next.
#[test]
fn ends_the_writer_after_a_call_that_dropped_while_it_sent() {
    const COUNT: usize = 1 << 17;
    on_hub(710, |session, clock| async move {
        let mut reader = reader(&session, &["value"]).await;
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        {
            frame(&mut writer, COUNT, 1_000, &[(2, 1)]);
            let mut write = pin!(writer.write(Time::Frame(Path::Live)));
            let waits = poll_fn(|cx| Poll::Ready(write.as_mut().poll(cx).is_pending()));
            assert!(waits.await, "the write waits for room");
        }
        let reset = Err(Error::Transport(transport::Error::Reset { code: Code(0) }));
        frame(&mut writer, 1, 0, &[(2, 2)]);
        assert_eq!(writer.write(Time::Node).await, reset);
        frame(&mut writer, 1, 0, &[(2, 3)]);
        assert_eq!(writer.write(Time::Node).await, reset);
        assert_eq!(writer.stored().await, reset.map(drop));
        let got = within(&clock, QUIET, reader.next()).await;
        assert!(got.is_none(), "{got:?}");
    });
}

/// Accepts the next write of `session` and opens it with `time` and `value`.
async fn opened(session: &transport::Session) -> Incoming {
    let mut write = accept_stream(session, |first| {
        Write::decode(first).expect("a write").body()
    })
    .await;
    let places = [place(1, STAMP, "time"), place(2, I64, "value")];
    start(&mut write, &places, 0, places.len()).await;
    write
}

/// Takes the head, the ends, and the body of a frame of 2 series.
async fn take_frame(write: &mut Incoming) {
    for _ in ["head", "ends", "body"] {
        let message = write.receiver.recv().await.expect("receives");
        message.expect("a message of the frame");
    }
}

/// The code that the program reset `write` with.
async fn reset(write: &mut Incoming) -> Code {
    match write.receiver.recv().await {
        Err(transport::Error::Reset { code }) => code,
        got => panic!("a reset, not {:?}", got.map(|got| got.map(|b| b.to_vec()))),
    }
}

/// A reply of a kind that no reply has, or a finish where a reply must come, ends the
/// writer with the error of the replies at each later call, and stops the stream as
/// `MALFORMED`.
#[test]
fn ends_the_writer_at_a_reply_that_wire_refuses() {
    let cases = [
        (711, Some(9), wire::hub::Error::Kind { kind: 9 }),
        (712, None, wire::hub::Error::Finished),
    ];
    for (seed, kind, error) in cases {
        by_hand(
            seed,
            move |session, node| async move {
                let mut write = opened(&session).await;
                take_frame(&mut write).await;
                let sender = write.sender.as_mut().expect("two-way");
                match kind {
                    Some(kind) => send(sender, &[kind]).await,
                    None => sender.finish().expect("finishes"),
                }
                assert_eq!(reset(&mut write).await, Code(MALFORMED));
                node.clock().sleep(QUIET).await;
            },
            move |session, node| {
                Box::pin(async move {
                    let mut writer =
                        session.writer(config(&["value"], 1)).await.expect("opens");
                    let ended = Err(Error::Reply(error));
                    frame(&mut writer, 1, 0, &[(2, 1)]);
                    assert_eq!(writer.write(Time::Node).await, ended);
                    frame(&mut writer, 1, 0, &[(2, 2)]);
                    assert_eq!(writer.write(Time::Node).await, ended);
                    assert_eq!(writer.stored().await, ended.map(drop));
                    node.clock().sleep(QUIET).await;
                })
            },
        );
    }
}

/// Each writer sends its own draft: the draft of one writer never goes to the channels
/// of another of the same types.
#[test]
fn sends_the_draft_of_each_writer_to_its_own_channels() {
    on_hub(713, |session, clock| async move {
        let mut reader = reader(&session, &["extra", "value"]).await;
        let mut value = session.writer(config(&["value"], 1)).await.expect("opens");
        let mut extra = session.writer(config(&["extra"], 1)).await.expect("opens");
        frame(&mut value, 1, 1_000, &[(2, 20)]);
        frame(&mut extra, 1, 2_000, &[(3, 30)]);
        assert_eq!(value.write(Time::Frame(Path::Live)).await, Ok(taken(0, 1)));
        drop(value);
        clock.sleep(QUIET).await;
        assert_eq!(extra.write(Time::Frame(Path::Live)).await, Ok(taken(1, 1)));
        for (at, other, sample) in [(2, 3, 20), (3, 2, 30)] {
            let got = reader.next().await.expect("a frame");
            let got = samples(&reader, &got);
            assert!(got.contains(&(key(at), vec![sample])), "{got:?}");
            assert!(
                !got.iter().any(|(key_of, _)| *key_of == key(other)),
                "{got:?}"
            );
        }
    });
}

/// A write with no draft is a bug of the program.
#[test]
#[should_panic(expected = "a write sends the draft of a call of `Writer::draft`")]
fn panics_at_a_write_with_no_draft() {
    on_hub(714, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 1)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        drop(writer.write(Time::Node).await);
    });
}

/// A write that gives `Unsent` drops the draft, so the next write with no new draft
/// panics.
#[test]
#[should_panic(expected = "a write sends the draft of a call of `Writer::draft`")]
fn panics_at_a_write_after_an_unsent_with_no_new_draft() {
    on_hub(720, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 1)]).set_count(0, 0);
        let unsent = Err(Error::Unsent {
            count: 0,
            form: Form::Raw,
        });
        assert_eq!(writer.write(Time::Node).await, unsent);
        drop(writer.write(Time::Node).await);
    });
}

/// A draft replaces the draft before it, and a write dropped before its first poll
/// leaves the draft held for the next write.
#[test]
fn sends_the_last_draft_after_a_write_dropped_before_its_first_poll() {
    on_hub(721, |session, _| async move {
        let mut reader = reader(&session, &["value"]).await;
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 1)]);
        frame(&mut writer, 1, 0, &[(2, 5)]);
        drop(writer.write(Time::Node));
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        let got = reader.next().await.expect("a frame");
        assert_eq!(samples(&reader, &got)[0], (key(2), vec![5]));
    });
}

/// A `stored` dropped before it gave leaves the writer whole: the next `stored` waits
/// for the same commit, and the next write gives its own outcome.
#[test]
fn keeps_the_writer_whole_after_a_dropped_stored() {
    on_hub(715, |session, _| async move {
        let mut writer = session.writer(config(&["value"], 1)).await.expect("opens");
        frame(&mut writer, 1, 0, &[(2, 1)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(0, 1)));
        {
            let mut stored = pin!(writer.stored());
            let waits =
                poll_fn(|cx| Poll::Ready(stored.as_mut().poll(cx).is_pending()));
            assert!(waits.await, "stored waits for the commit");
        }
        assert_eq!(writer.stored().await, Ok(()));
        frame(&mut writer, 1, 0, &[(2, 2)]);
        assert_eq!(writer.write(Time::Node).await, Ok(taken(1, 1)));
    });
}

/// A writer whose head and ends of a frame no block of `Config::pool` holds fails at
/// the open with `Error::Pool`, and the program drops the stream.
#[test]
fn refuses_a_writer_whose_frame_lead_no_block_of_its_pool_holds() {
    const CHANNELS: usize = 127;
    let names: Vec<String> = (0..CHANNELS).map(|at| format!("c{at:03}")).collect();
    let places: Vec<Place> = [place(1, STAMP, "time")]
        .into_iter()
        .chain(names.iter().zip(2..).map(|(n, at)| place(at, I64, n)))
        .collect();
    raw(
        716,
        move |session, mut hello, node| async move {
            admit(&mut hello, &node.clock()).await;
            let mut write = accept_stream(&session, |first| {
                Write::decode(first).expect("a write").body()
            })
            .await;
            start(&mut write, &places, 0, places.len()).await;
            assert_eq!(reset(&mut write).await, Code(0));
            node.clock().sleep(QUIET).await;
        },
        move |node, tasks, at| async move {
            let config = block::Config::new(
                u64::try_from(block::footprint(1 << 10)).expect("a usize fits a u64"),
            )
            .expect("the budget fits");
            let pool = block::Pool::new(
                config.clone(),
                block::Heap::new(config.reservation()),
            );
            let session = connect_with(&node, tasks, at, AGENT, Rc::new(pool), ample())
                .await
                .expect("connects");
            let channels = names.iter().map(|n| name(n)).collect();
            let config = Config {
                authority: Authority(1),
                lease: None,
                channels,
            };
            let got = session.writer(config).await.map(drop);
            let pool = block::Error::TooLarge {
                requested: 11 + 8 * (CHANNELS + 1),
                largest: 1024,
            };
            assert_eq!(got, Err(Error::Pool(pool)));
            node.clock().sleep(QUIET).await;
        },
    );
}
