//! `wire::hub::write::Frames`, driven as a caller drives it by `Frames::due`, never
//! panics. Each head that it gives encodes back to its message, so it gives no head
//! that `Head::encode` panics on. Each error is one that `decode` lists for the part
//! that was due. Each frame has the time, form, and count of its head, and a run that
//! receives each body message in place gives the same steps as a run that gives it
//! whole to `decode`.
//!
//! Input: one byte, whose low 3 bits are the places of the stream less 1, then the
//! messages from the program (`fuzz::messages`).

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use types::channel;
use types::frame::key_set::KeySet;
use types::frame::{Form, Path, Range};
use types::sample::{Scalar, Type};
use wire::hub::Error;
use wire::hub::write::{Due, Frames, Head, Step, Time};

/// What one step gave, with a frame's series copied out.
#[derive(Debug, PartialEq)]
enum Seen {
    Head(Head),
    More,
    Frame {
        time: Time,
        form: Form,
        range: Option<Range>,
        series: Vec<Option<Vec<u8>>>,
    },
    /// The error of `decode` or `received`, which stops the stream.
    Message(Error),
    /// The error of a draft's layout, which stops the stream.
    Layout(types::frame::Error),
    /// The pool had no block for the draft.
    Pool,
}

/// An index, then `places - 1` channels of `I64`, in place order.
fn set(places: u8) -> Arc<KeySet> {
    let channels = (1..=places).map(|n| {
        let data = if n == 1 { Scalar::Stamp } else { Scalar::I64 };
        (channel::Key::from_u128(u128::from(n)), Type::Scalar(data))
    });
    Arc::new(KeySet::ordered(channels, 0).expect("no key repeats"))
}

fn pool() -> block::Pool {
    let config = block::Config::new(1 << 16).expect("the budget fits");
    let memory = block::Heap::new(config.reservation());
    block::Pool::new(config, memory)
}

impl Seen {
    fn of(step: Step, places: u8) -> Self {
        match step {
            Step::Head(head) => Self::Head(head),
            Step::More => Self::More,
            Step::Frame { draft, time } => {
                let form = draft.form();
                let range = draft.range(0);
                let frame = draft.freeze(Path::Live);
                Self::Frame {
                    time,
                    form,
                    range,
                    series: (0..usize::from(places))
                        .map(|entry| frame.series(entry).map(<[u8]>::to_vec))
                        .collect(),
                }
            }
        }
    }
}

/// Checks that `error` is one that `decode` lists for a head when `head` is due, else
/// for a part of the frame.
fn check_error(error: Error, head: bool) {
    let listed = if head {
        matches!(
            error,
            Error::Empty
                | Error::Kind { .. }
                | Error::Length { .. }
                | Error::Path { .. }
                | Error::Form { .. }
                | Error::Samples
                | Error::Series
                | Error::Stamp { .. }
                | Error::Places { .. }
        )
    } else {
        matches!(
            error,
            Error::Empty
                | Error::Length { .. }
                | Error::Run { .. }
                | Error::Body { .. }
        )
    };
    assert!(
        listed,
        "a {} gave {error:?}",
        if head { "head" } else { "part" }
    );
}

/// The steps of a stream of `places` that reads `bytes`, each body message received
/// in place when `in_place` and it fits the body.
fn run(places: u8, bytes: &[u8], in_place: bool) -> Vec<Seen> {
    let pool = pool();
    let mut frames = Frames::new(set(places));
    let mut messages = fuzz::messages(bytes);
    let mut seen = Vec::new();
    // A head is due at the start and after each frame.
    let mut head = true;
    loop {
        let step = match frames.due() {
            Err(error) => {
                assert_eq!(
                    frames.due().map(drop),
                    Err(error.clone()),
                    "the layout changed"
                );
                seen.push(Seen::Layout(error));
                return seen;
            }
            Ok(Due::Draft { layout, form }) => match layout.draft(&pool, form) {
                Ok(draft) => Ok(frames.draft(draft)),
                Err(_) => {
                    seen.push(Seen::Pool);
                    return seen;
                }
            },
            Ok(Due::Body(rest)) => {
                let Some(message) = messages.next() else {
                    return seen;
                };
                match rest.get_mut(..message.len()) {
                    Some(into) if in_place => {
                        into.copy_from_slice(message);
                        frames.received(message.len())
                    }
                    _ => frames.decode(message),
                }
            }
            Ok(Due::Message) => {
                let Some(message) = messages.next() else {
                    return seen;
                };
                let step = frames.decode(message);
                if let Ok(Step::Head(given)) = &step {
                    let mut out = [0; Head::LEN];
                    given.encode(&mut out);
                    assert_eq!(&out[..], message, "the head changed");
                }
                step
            }
        };
        match step {
            Ok(step) => {
                head = matches!(step, Step::Frame { .. });
                seen.push(Seen::of(step, places));
            }
            Err(error) => {
                check_error(error, head);
                seen.push(Seen::Message(error));
                return seen;
            }
        }
    }
}

/// Checks that each frame of `seen` has the time, form, and count of the head before
/// it.
fn check(seen: &[Seen]) {
    let mut head = None;
    for step in seen {
        match step {
            Seen::Head(next) => head = Some(*next),
            Seen::Frame {
                time, form, range, ..
            } => {
                let head = head.take().expect("a frame came with no head");
                assert_eq!(*time, head.time, "the frame changed its time");
                assert_eq!(*form, head.form, "the frame changed its form");
                let count = Some(Range {
                    seq: 0,
                    count: head.count,
                });
                assert_eq!(*range, count, "the frame changed its count");
            }
            _ => {}
        }
    }
}

fuzz_target!(|bytes: &[u8]| {
    let Some((&first, bytes)) = bytes.split_first() else {
        return;
    };
    let places = (first & 0b111) + 1;
    let whole = run(places, bytes, false);
    check(&whole);
    assert_eq!(run(places, bytes, true), whole, "the two runs differ");
});
