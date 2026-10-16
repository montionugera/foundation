//! `wire::hub::Frames`, opened by the first message and driven as a caller drives it by
//! `Frames::due`, never panics. Each frame has the path and range of its head, and a
//! run that receives each body message in place gives the same steps as a run that
//! gives it whole to `decode`.
//!
//! Input: one byte, whose low 3 bits are the places of the session less 1 and whose
//! bit 3 makes the session latest, then the messages from the home (`fuzz::messages`).

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use std::sync::Arc;

use libfuzzer_sys::fuzz_target;
use types::channel;
use types::frame::key_set::{Group, Interner, KeySet};
use types::frame::{Form, Path, Range};
use types::sample::{Scalar, Type};
use wire::hub::{Due, Error, Frames, Head, Mode, Open, Reader, Step};

/// What one step gave, with a frame's series copied out.
#[derive(Debug, PartialEq)]
enum Seen {
    Head(Head),
    More,
    Frame {
        path: Path,
        range: Option<Range>,
        series: Vec<Option<Vec<u8>>>,
    },
    Behind,
    /// The error of `Frames::open`, `decode`, or `received`, which stops the stream.
    Message(Error),
    /// The error of a draft's layout, which stops the stream.
    Layout(types::frame::Error),
    /// The pool had no block for the draft.
    Pool,
    /// The input ended, with the error of the stream that the home finished there.
    Finished(Error),
}

/// An index, then `places - 1` channels of `I64`.
fn set(places: u8) -> Arc<KeySet> {
    let data: Vec<_> = (1..places)
        .map(|n| {
            let key = channel::Key::from_u128(u128::from(n) + 1);
            (key, Type::Scalar(Scalar::I64))
        })
        .collect();
    Interner::new().intern(&[Group {
        index: channel::Key::from_u128(1),
        data: &data,
    }])
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
            Step::Frame(frame) => Self::Frame {
                path: frame.path(),
                range: frame.range(0),
                series: (0..usize::from(places))
                    .map(|entry| frame.series(entry).map(<[u8]>::to_vec))
                    .collect(),
            },
            Step::Behind => Self::Behind,
        }
    }
}

/// The steps of a session of `places` in `mode` that reads `bytes`, each body message
/// received in place when `in_place` and it fits the body.
fn run(places: u8, mode: Mode, bytes: &[u8], in_place: bool) -> Vec<Seen> {
    let pool = pool();
    let open = Open {
        mode,
        channels: u32::from(places),
    };
    let mut messages = fuzz::messages(bytes);
    let mut seen = Vec::new();
    let Some(first) = messages.next() else {
        return seen;
    };
    let mut frames = match Frames::open(Reader::new(&open), set(places), first) {
        Ok(frames) => frames,
        Err(error) => {
            seen.push(Seen::Message(error));
            return seen;
        }
    };
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
            Ok(Due::Draft(layout)) => match layout.draft(&pool, Form::Encoded) {
                Ok(draft) => Ok(frames.draft(draft)),
                Err(_) => {
                    seen.push(Seen::Pool);
                    return seen;
                }
            },
            Ok(Due::Body(rest)) => {
                let Some(message) = messages.next() else {
                    break;
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
                    break;
                };
                frames.decode(message)
            }
        };
        match step {
            Ok(Step::Behind) => {
                seen.push(Seen::Behind);
                return seen;
            }
            Ok(step) => seen.push(Seen::of(step, places)),
            Err(error) => {
                seen.push(Seen::Message(error));
                return seen;
            }
        }
    }
    seen.push(Seen::Finished(frames.finished()));
    seen
}

/// Checks that each frame of `seen` has the path and range of the head before it.
fn check(seen: &[Seen]) {
    let mut head = None;
    for step in seen {
        match step {
            Seen::Head(next) => head = Some(*next),
            Seen::Frame { path, range, .. } => {
                let head = head.take().expect("a frame came with no head");
                assert_eq!(*path, head.path, "the frame changed its path");
                assert_eq!(*range, Some(head.range), "the frame changed its range");
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
    let mode = if first & 0b1000 == 0 {
        Mode::Complete { limit_bytes: 0 }
    } else {
        Mode::Latest
    };
    let whole = run(places, mode, bytes, false);
    check(&whole);
    assert_eq!(run(places, mode, bytes, true), whole, "the two runs differ");
});
