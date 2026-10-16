use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Frame, Layout};

use super::{Error, FromHome, Head, Reader};

/// The frames of a reader session, built from its replies from `Opened` on, at the
/// reader's node and at the program. The caller receives each message and makes each
/// draft from its own pool. The entries of each frame index the key set.
#[derive(Debug)]
pub struct Frames {
    reader: Reader,
    set: Arc<KeySet>,
    head: Option<Head>,
    /// The entry and end of each series of the frame that arrives.
    ends: Vec<(usize, usize)>,
    /// The ends are whole and the draft is not given.
    drafting: bool,
    draft: Option<Draft>,
}

/// What [`Frames`] waits for next.
#[derive(Debug)]
pub enum Due<'f> {
    /// A whole message, for [`Frames::decode`].
    Message,
    /// Body bytes. Receive into the slice, the rest of the draft's body, and give the
    /// length to [`Frames::received`]. A message over the slice goes to
    /// [`Frames::decode`] whole, which gives its error.
    Body(&'f mut [u8]),
    /// A draft of this layout from the caller's pool, for [`Frames::draft`].
    Draft(Layout<'f>),
}

/// What one input gave.
#[derive(Debug)]
pub enum Step {
    /// The head of a frame. The run of its ends follows.
    Head(Head),
    /// Part of a run of ends or of a body, or a draft that the body fills.
    More,
    /// A whole frame.
    Frame(Frame),
    /// The home ended the session, and no message follows. No case gives it; #2790
    /// removes it.
    Behind,
}

impl Frames {
    /// The frames of the session that `reader` decodes, opened by `message`, the
    /// first message of the session, or `None` when the stream ended before it. The
    /// places of the session are the entries of `set`, in order. Returns the frames
    /// and the open position of `Opened`.
    ///
    /// # Errors
    ///
    /// [`Error::Finished`] when `message` is `None`, each [`Error`] of
    /// [`Reader::decode`], and [`Error::Unopened`] for any other message. The caller
    /// then stops the stream as `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When `set` has more than one group, or `reader` is past `Opened` and decodes
    /// `message`.
    pub fn open(
        mut reader: Reader,
        set: Arc<KeySet>,
        message: Option<&[u8]>,
    ) -> Result<(Self, u64), Error> {
        assert_eq!(set.groups().len(), 1, "a session is on one index");
        let Some(message) = message else {
            return Err(reader
                .end()
                .expect_err("invariant: a reader before opened may not end"));
        };
        match reader.decode(message)? {
            FromHome::Opened { live } => {
                let frames = Self {
                    reader,
                    set,
                    head: None,
                    ends: Vec::new(),
                    drafting: false,
                    draft: None,
                };
                Ok((frames, live))
            }
            _ => panic!("a reader before opened gives only opened"),
        }
    }

    /// The key set of each frame.
    #[must_use]
    pub fn set(&self) -> &Arc<KeySet> {
        &self.set
    }

    /// What the frames wait for. Ask it before each input: a call that dropped
    /// between two inputs resumes from it.
    ///
    /// # Errors
    ///
    /// When a draft is due, each [`types::frame::Error`] of [`Layout::from_ends`]. The
    /// caller then stops the stream as `MALFORMED`.
    #[inline]
    pub fn due(&mut self) -> Result<Due<'_>, types::frame::Error> {
        if self.drafting {
            return Layout::from_ends(&self.set, &self.ends).map(Due::Draft);
        }
        Ok(match (self.reader.body(), self.draft.as_mut()) {
            (Some(start), Some(draft)) => Due::Body(rest(draft, start)),
            (None, _) => Due::Message,
            (Some(_), None) => unreachable!("invariant: a body has a draft"),
        })
    }

    /// Takes the next message, whole. A body message is copied into the draft.
    ///
    /// # Errors
    ///
    /// Each [`Error`] of [`Reader::decode`]. The caller then stops the stream as
    /// `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] gives [`Due::Draft`].
    #[inline]
    pub fn decode(&mut self, message: &[u8]) -> Result<Step, Error> {
        assert!(!self.drafting, "a draft is due before the next message");
        if let Some(start) = self.reader.body() {
            let last = last(self.reader.decode(message))?;
            let draft = self.draft.as_mut().expect("invariant: a body has a draft");
            prefix(rest(draft, start), message.len()).copy_from_slice(message);
            return Ok(self.received_body(last));
        }
        match self.reader.decode(message)? {
            FromHome::Head(head) => {
                self.head = Some(head);
                self.ends.clear();
                Ok(Step::Head(head))
            }
            FromHome::Ends { ends, last } => {
                self.ends
                    .extend(ends.map(|(place, end)| (to_usize(place), to_usize(end))));
                self.drafting = last;
                Ok(Step::More)
            }
            FromHome::Behind => Ok(Step::Behind),
            FromHome::Opened { .. } | FromHome::Body { .. } => {
                unreachable!("invariant: an open decoder gives no opened or body here")
            }
        }
    }

    /// Takes a body message of `len` bytes that the caller wrote at the start of the
    /// slice of [`Due::Body`].
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `len` is 0. The caller then stops the stream as
    /// `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] does not give [`Due::Body`], or `len` is over its slice.
    #[inline]
    pub fn received(&mut self, len: usize) -> Result<Step, Error> {
        assert!(!self.drafting, "a body is due");
        let last = self.reader.received(len)?;
        Ok(self.received_body(last))
    }

    /// Takes the draft, which is of the layout of [`Due::Draft`]. Gives the frame when
    /// it has no body, else [`Step::More`].
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] does not give [`Due::Draft`].
    #[inline]
    pub fn draft(&mut self, draft: Draft) -> Step {
        assert!(self.drafting, "no draft is due");
        self.drafting = false;
        self.draft = Some(draft);
        match self.reader.body() {
            Some(_) => Step::More,
            None => Step::Frame(self.freeze()),
        }
    }

    /// The error of a stream that the home finished here.
    ///
    /// # Panics
    ///
    /// After [`Step::Behind`], where the stream may end.
    #[must_use]
    pub fn finished(&self) -> Error {
        self.reader
            .end()
            .expect_err("a stream may end after Behind")
    }

    /// The step of the body message that arrived, a frame when it is the `last`.
    fn received_body(&mut self, last: bool) -> Step {
        if last {
            Step::Frame(self.freeze())
        } else {
            Step::More
        }
    }

    /// The frame of the head and the draft that arrived.
    fn freeze(&mut self) -> Frame {
        let head = self.head.take().expect("invariant: a frame has a head");
        let mut draft = self.draft.take().expect("invariant: a frame has a draft");
        draft.set_count(0, head.range.count);
        draft.set_seq(0, head.range.seq);
        draft.freeze(head.path)
    }
}

/// The body of `draft` from `start` on.
fn rest(draft: &mut Draft, start: usize) -> &mut [u8] {
    let body = draft.body_mut();
    let len = body.len();
    body.get_mut(start..).unwrap_or_else(|| {
        panic!(
            "invariant: a body that starts at {start} is inside its {len} byte draft"
        )
    })
}

/// The first `len` bytes of `rest`.
fn prefix(rest: &mut [u8], len: usize) -> &mut [u8] {
    let room = rest.len();
    rest.get_mut(..len).unwrap_or_else(|| {
        panic!(
            "a body message of {len} bytes is over the {room} bytes left of its body"
        )
    })
}

/// Whether the body message that `decoded` gave is the last of its body.
fn last(decoded: Result<FromHome<'_>, Error>) -> Result<bool, Error> {
    match decoded? {
        FromHome::Body { last, .. } => Ok(last),
        _ => unreachable!("invariant: inside a body the decoder gives body bytes"),
    }
}

pub(super) fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("invariant: a usize holds a u32")
}

#[cfg(test)]
mod tests {
    use types::frame::key_set::{Group, Interner};
    use types::frame::{BadEnd, Form, Path, Range};
    use types::sample::{Scalar, Type};

    use super::*;
    use crate::hub::tests::{encode_ends, encode_reply, key};
    use crate::hub::{BEHIND, HEAD, Mode, OPENED, Open, Reply};

    /// Index key 1 (entry 0), then an `I64` of key 2 (entry 1).
    fn create_set() -> Arc<KeySet> {
        Interner::new().intern(&[Group {
            index: key(1),
            data: &[(key(2), Type::Scalar(Scalar::I64))],
        }])
    }

    fn create_pool() -> block::Pool {
        let config = block::Config::new(1 << 16).expect("the budget fits");
        let memory = block::Heap::new(config.reservation());
        block::Pool::new(config, memory)
    }

    /// A reader of a session of 2 places, latest when `latest`, before `Opened`.
    fn create_reader(latest: bool) -> Reader {
        let mode = if latest {
            Mode::Latest
        } else {
            Mode::Complete { limit_bytes: 0 }
        };
        Reader::new(&Open { mode, channels: 2 })
    }

    /// The `Opened` reply at the open position `live`.
    fn opened_at(live: u64) -> Vec<u8> {
        encode_reply(Reply::Opened { live })
    }

    /// The frames of an opened session of 2 places, latest when `latest`.
    fn opened(latest: bool) -> Frames {
        let opened = opened_at(0);
        let (frames, _) =
            Frames::open(create_reader(latest), create_set(), Some(&opened))
                .expect("opened");
        frames
    }

    /// A live head of `count` samples from `seq` with `series` series.
    fn head(seq: u64, count: u32, series: u32) -> Vec<u8> {
        encode_reply(Reply::Head(Head {
            path: Path::Live,
            range: Range { seq, count },
            series,
        }))
    }

    /// Gives `frames` the draft that is due.
    fn draft(frames: &mut Frames, pool: &block::Pool) -> Step {
        let Ok(Due::Draft(layout)) = frames.due() else {
            panic!("a draft is due");
        };
        let draft = layout.draft(pool, Form::Encoded).expect("a block");
        frames.draft(draft)
    }

    /// Takes a head and the ends of 2 series of 16 bytes each.
    fn ends(frames: &mut Frames) {
        let step = frames.decode(&head(4, 2, 2));
        assert!(matches!(step, Ok(Step::Head(h)) if h.series == 2));
        let step = frames.decode(&encode_ends(&[(0, 16)]));
        assert!(matches!(step, Ok(Step::More)));
        assert!(matches!(frames.due(), Ok(Due::Message)));
        let step = frames.decode(&encode_ends(&[(1, 32)]));
        assert!(matches!(step, Ok(Step::More)));
        assert!(matches!(frames.due(), Ok(Due::Draft(_))));
    }

    fn check(frame: &Frame, body: &[u8]) {
        assert_eq!(frame.path(), Path::Live);
        assert_eq!(frame.range(0), Some(Range { seq: 4, count: 2 }));
        assert_eq!(frame.series(0), Some(&body[..16]));
        assert_eq!(frame.series(1), Some(&body[16..]));
    }

    #[test]
    fn gives_the_open_position_of_opened() {
        let opened =
            Frames::open(create_reader(false), create_set(), Some(&opened_at(7)));
        assert!(matches!(opened, Ok((_, 7))));
    }

    #[test]
    fn opens_the_session_once() {
        let mut frames = opened(false);
        assert!(matches!(frames.due(), Ok(Due::Message)));
        assert_eq!(
            frames.decode(&opened_at(0)).map(drop),
            Err(Error::Reopen { kind: OPENED })
        );
    }

    #[test]
    fn refuses_a_first_message_that_is_not_opened() {
        let open = |message: Option<&[u8]>| {
            Frames::open(create_reader(false), create_set(), message).map(drop)
        };
        let head = head(4, 2, 2);
        assert_eq!(open(Some(&head)), Err(Error::Unopened { kind: HEAD }));
        let behind = encode_reply(Reply::Behind);
        assert_eq!(open(Some(&behind)), Err(Error::Unopened { kind: BEHIND }));
        assert_eq!(open(Some(&[])), Err(Error::Empty));
        assert_eq!(open(None), Err(Error::Finished));
    }

    #[test]
    #[should_panic(expected = "a session is on one index")]
    fn panics_at_a_set_of_two_groups() {
        let set = Interner::new().intern(&[
            Group {
                index: key(1),
                data: &[],
            },
            Group {
                index: key(3),
                data: &[],
            },
        ]);
        let _frames = Frames::open(create_reader(false), set, Some(&opened_at(0)));
    }

    #[test]
    #[should_panic(expected = "a reader before opened gives only opened")]
    fn panics_at_a_reader_past_opened() {
        let mut reader = create_reader(false);
        reader.decode(&opened_at(0)).expect("opened");
        let _frames = Frames::open(reader, create_set(), Some(&head(4, 2, 2)));
    }

    #[test]
    fn builds_a_frame_from_whole_messages() {
        let (pool, mut frames) = (create_pool(), opened(false));
        let body: Vec<u8> = (0..32).collect();
        ends(&mut frames);
        assert!(matches!(draft(&mut frames, &pool), Step::More));
        assert!(matches!(frames.decode(&body[..10]), Ok(Step::More)));
        let Ok(Step::Frame(frame)) = frames.decode(&body[10..]) else {
            panic!("a frame");
        };
        check(&frame, &body);
        assert!(matches!(frames.due(), Ok(Due::Message)));
    }

    #[test]
    fn builds_a_frame_from_bytes_received_in_place() {
        let (pool, mut frames) = (create_pool(), opened(false));
        let body: Vec<u8> = (100..132).collect();
        ends(&mut frames);
        draft(&mut frames, &pool);
        let Ok(Due::Body(rest)) = frames.due() else {
            panic!("a body is due");
        };
        assert_eq!(rest.len(), 32);
        rest[..20].copy_from_slice(&body[..20]);
        assert!(matches!(frames.received(20), Ok(Step::More)));
        let Ok(Due::Body(rest)) = frames.due() else {
            panic!("a body is due");
        };
        assert_eq!(rest.len(), 12);
        rest.copy_from_slice(&body[20..]);
        let Ok(Step::Frame(frame)) = frames.received(12) else {
            panic!("a frame");
        };
        check(&frame, &body);
    }

    #[test]
    fn gives_a_frame_with_no_body_at_its_draft() {
        let (pool, mut frames) = (create_pool(), opened(false));
        frames.decode(&head(4, 2, 2)).expect("a head");
        let step = frames.decode(&encode_ends(&[(0, 0), (1, 0)]));
        assert!(matches!(step, Ok(Step::More)));
        let Step::Frame(frame) = draft(&mut frames, &pool) else {
            panic!("a frame");
        };
        assert_eq!(frame.series(0), Some(&[][..]));
        assert_eq!(frame.range(0), Some(Range { seq: 4, count: 2 }));
        assert!(matches!(frames.due(), Ok(Due::Message)));
        assert_eq!(frames.finished(), Error::Finished);
    }

    /// A call that drops at its wait for the pool finds the draft due again.
    #[test]
    fn gives_a_due_draft_again() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        assert!(matches!(frames.due(), Ok(Due::Draft(_))));
        assert!(matches!(draft(&mut frames, &pool), Step::More));
    }

    #[test]
    fn builds_each_frame_from_its_own_ends() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        let first: Vec<u8> = (0..32).collect();
        assert!(matches!(frames.decode(&first), Ok(Step::Frame(_))));
        frames.decode(&head(6, 1, 1)).expect("a head");
        frames.decode(&encode_ends(&[(0, 8)])).expect("ends");
        draft(&mut frames, &pool);
        let Ok(Step::Frame(frame)) = frames.decode(&[7; 8]) else {
            panic!("a frame");
        };
        assert_eq!(frame.range(0), Some(Range { seq: 6, count: 1 }));
        assert_eq!(frame.series(0), Some(&[7; 8][..]));
        assert_eq!(frame.series(1), None);
    }

    #[test]
    fn refuses_ends_that_break_the_layout() {
        let mut frames = opened(false);
        frames.decode(&head(4, 2, 1)).expect("a head");
        frames.decode(&encode_ends(&[(1, 8)])).expect("ends");
        assert_eq!(
            frames.due().map(drop),
            Err(types::frame::Error::IndexAbsent { entry: 1, index: 0 })
        );
        let mut frames = opened(false);
        frames.decode(&head(4, 2, 2)).expect("a head");
        frames
            .decode(&encode_ends(&[(0, 16), (1, 8)]))
            .expect("ends");
        assert_eq!(
            frames.due().map(drop),
            Err(types::frame::Error::End(BadEnd::Before {
                end: 8,
                start: 16
            }))
        );
    }

    #[test]
    fn refuses_a_body_message_past_the_body() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        frames.decode(&[0; 30]).expect("body");
        assert_eq!(
            frames.decode(&[0; 3]).map(drop),
            Err(Error::Body { len: 3, remain: 2 })
        );
    }

    #[test]
    fn refuses_an_empty_body_message_received_in_place() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        assert_eq!(frames.received(0).map(drop), Err(Error::Empty));
        assert!(matches!(frames.due(), Ok(Due::Body(rest)) if rest.len() == 32));
    }

    #[test]
    fn gives_behind_and_the_error_of_each_finish() {
        let (pool, mut frames) = (create_pool(), opened(false));
        assert_eq!(frames.finished(), Error::Finished);
        ends(&mut frames);
        draft(&mut frames, &pool);
        frames.decode(&[0; 30]).expect("body");
        assert_eq!(frames.finished(), Error::Unfinished { remain: 2 });
        frames.decode(&[0; 2]).expect("body");
        let behind = encode_reply(Reply::Behind);
        assert!(matches!(frames.decode(&behind), Ok(Step::Behind)));
        assert_eq!(frames.decode(&behind).map(drop), Err(Error::Ended));
        let mut frames = opened(true);
        assert_eq!(
            frames.decode(&behind).map(drop),
            Err(Error::Latest { kind: 3 })
        );
    }

    #[test]
    #[should_panic(expected = "a draft is due before the next message")]
    fn panics_at_a_message_before_a_due_draft() {
        let mut frames = opened(false);
        ends(&mut frames);
        let _step = frames.decode(&[0; 4]);
    }

    #[test]
    #[should_panic(expected = "no draft is due")]
    fn panics_at_a_draft_that_is_not_due() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        let Ok(Due::Draft(layout)) = frames.due() else {
            panic!("a draft is due");
        };
        let twice = layout.draft(&pool, Form::Encoded).expect("a block");
        draft(&mut frames, &pool);
        frames.draft(twice);
    }

    #[test]
    #[should_panic(expected = "a body is due")]
    fn panics_at_bytes_received_with_no_body_due() {
        let _step = opened(false).received(1);
    }

    #[test]
    #[should_panic(expected = "a body is due")]
    fn panics_at_bytes_received_before_a_due_draft() {
        let mut frames = opened(false);
        ends(&mut frames);
        let _step = frames.received(1);
    }

    #[test]
    #[should_panic(expected = "a body message of 33 bytes is over the 32 bytes left")]
    fn panics_at_bytes_received_past_the_body() {
        let (pool, mut frames) = (create_pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        let _step = frames.received(33);
    }

    #[test]
    #[should_panic(expected = "a stream may end after Behind")]
    fn panics_at_a_finish_after_behind() {
        let mut frames = opened(false);
        frames.decode(&encode_reply(Reply::Behind)).expect("behind");
        let _error = frames.finished();
    }
}
