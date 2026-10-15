use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Frame, Layout};

use super::{Error, FromHome, Head, Reader};

/// The frames of a reader session, built from its replies after `Opened`, at the
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

/// What one message gave.
#[derive(Debug)]
pub enum Step {
    /// The head of a frame. The run of its ends follows.
    Head(Head),
    /// Part of a run of ends, or of a body.
    More,
    /// The run of ends is whole. Make a draft of [`Frames::layout`], and give it to
    /// [`Frames::draft`].
    Draft,
    /// A whole frame.
    Frame(Frame),
    /// The session reached a frame that the buffer lost, so the home ended it. No
    /// message follows.
    Behind,
}

impl Frames {
    /// The frames of the session that `reader` decodes, after its `Opened`. The
    /// places of the session are the entries of `set`, in order.
    #[must_use]
    pub fn new(reader: Reader, set: Arc<KeySet>) -> Self {
        Self {
            reader,
            set,
            head: None,
            ends: Vec::new(),
            drafting: false,
            draft: None,
        }
    }

    /// The key set of each frame.
    #[must_use]
    pub fn set(&self) -> &Arc<KeySet> {
        &self.set
    }

    /// Where the next message goes when it is body bytes: the rest of the body in the
    /// draft. `None` when the next message is a reply or ends, or before the draft.
    /// Receive into it, then give the length to [`Frames::received`].
    pub fn body(&mut self) -> Option<&mut [u8]> {
        let start = self.reader.body()?;
        let draft = self.draft.as_mut()?;
        Some(rest(draft, start))
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
    /// After [`Step::Draft`], until [`Frames::draft`].
    pub fn decode(&mut self, message: &[u8]) -> Result<Step, Error> {
        assert!(!self.drafting, "a draft is due before the next message");
        if let Some(start) = self.reader.body() {
            let last = last(self.reader.decode(message))?;
            let draft = self.draft.as_mut().expect("invariant: a body has a draft");
            prefix(rest(draft, start), message.len()).copy_from_slice(message);
            return Ok(self.received_body(last).map_or(Step::More, Step::Frame));
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
                Ok(if last { Step::Draft } else { Step::More })
            }
            FromHome::Behind => Ok(Step::Behind),
            FromHome::Opened | FromHome::Body { .. } => {
                unreachable!("invariant: the decoder gives no opened or body here")
            }
        }
    }

    /// Takes a body message of `len` bytes that the caller wrote at the start of
    /// [`Frames::body`]. Gives the frame when the body is whole.
    ///
    /// # Errors
    ///
    /// Each [`Error`] of [`Reader::decode`].
    ///
    /// # Panics
    ///
    /// When no body is due, or `len` is over the length of [`Frames::body`].
    pub fn received(&mut self, len: usize) -> Result<Option<Frame>, Error> {
        let start = self.reader.body().expect("a body is due");
        let draft = self.draft.as_mut().expect("a body is due");
        let last = last(self.reader.decode(prefix(rest(draft, start), len)))?;
        Ok(self.received_body(last))
    }

    /// The layout of the frame whose ends are whole, after [`Step::Draft`]. It can be
    /// asked again, as by a call that dropped while its draft waited for the pool.
    ///
    /// # Errors
    ///
    /// Each [`types::frame::Error`] of [`Layout::from_ends`]. The caller then stops
    /// the stream as `MALFORMED`.
    pub fn layout(&self) -> Result<Layout<'_>, types::frame::Error> {
        Layout::from_ends(&self.set, &self.ends)
    }

    /// Takes the draft of [`Frames::layout`]. Gives the frame when it has no body.
    ///
    /// # Panics
    ///
    /// When no draft is due.
    pub fn draft(&mut self, draft: Draft) -> Option<Frame> {
        assert!(self.drafting, "no draft is due");
        self.drafting = false;
        self.draft = Some(draft);
        match self.reader.body() {
            Some(_) => None,
            None => Some(self.freeze()),
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
            .expect_err("invariant: a session reads no message after Behind")
    }

    /// The frame when the body message that arrived is the `last`.
    fn received_body(&mut self, last: bool) -> Option<Frame> {
        last.then(|| self.freeze())
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
    draft
        .body_mut()
        .get_mut(start..)
        .expect("invariant: a body starts inside its draft")
}

/// The first `len` bytes of `rest`.
fn prefix(rest: &mut [u8], len: usize) -> &mut [u8] {
    rest.get_mut(..len)
        .expect("a body message fits the rest of its body")
}

/// Whether the body message that `decoded` gave is the last of its body.
fn last(decoded: Result<FromHome<'_>, Error>) -> Result<bool, Error> {
    match decoded? {
        FromHome::Body { last, .. } => Ok(last),
        _ => unreachable!("invariant: inside a body the decoder gives body bytes"),
    }
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("invariant: a usize holds a u32")
}

#[cfg(test)]
mod tests {
    use types::frame::key_set::{Group, Interner};
    use types::frame::{BadEnd, Form, Path, Range};
    use types::sample::{Scalar, Type};

    use super::*;
    use crate::hub::tests::{encode_ends, encode_reply, key};
    use crate::hub::{Mode, OPENED, Open, Reply};

    /// Index key 1 (entry 0), then an `I64` of key 2 (entry 1).
    fn set() -> Arc<KeySet> {
        Interner::new().intern(&[Group {
            index: key(1),
            data: &[(key(2), Type::Scalar(Scalar::I64))],
        }])
    }

    fn pool() -> block::Pool {
        let config = block::Config::new(1 << 16).expect("the budget fits");
        let memory = block::Heap::new(config.reservation());
        block::Pool::new(config, memory)
    }

    /// The frames of an opened session of 2 places, latest when `latest`.
    fn opened(latest: bool) -> Frames {
        let mode = if latest {
            Mode::Latest
        } else {
            Mode::Complete { limit_bytes: 0 }
        };
        let mut reader = Reader::new(&Open { mode, channels: 2 });
        reader.decode(&[OPENED]).expect("the session opens");
        Frames::new(reader, set())
    }

    /// A live head of 2 samples from seq 4 with `series` series.
    fn head(series: u32) -> Vec<u8> {
        encode_reply(Reply::Head(Head {
            path: Path::Live,
            range: Range { seq: 4, count: 2 },
            series,
        }))
    }

    /// Gives `frames` a draft of its layout.
    fn draft(frames: &mut Frames, pool: &block::Pool) -> Option<Frame> {
        let draft = frames
            .layout()
            .expect("a layout")
            .draft(pool, Form::Encoded)
            .expect("a block");
        frames.draft(draft)
    }

    /// Takes a head and the ends of 2 series of 16 bytes each.
    fn ends(frames: &mut Frames) {
        assert!(matches!(frames.decode(&head(2)), Ok(Step::Head(h)) if h.series == 2));
        assert!(matches!(
            frames.decode(&encode_ends(&[(0, 16)])),
            Ok(Step::More)
        ));
        assert!(frames.body().is_none());
        assert!(matches!(
            frames.decode(&encode_ends(&[(1, 32)])),
            Ok(Step::Draft)
        ));
        assert!(frames.body().is_none());
    }

    fn check(frame: &Frame, body: &[u8]) {
        assert_eq!(frame.path(), Path::Live);
        assert_eq!(frame.range(0), Some(Range { seq: 4, count: 2 }));
        assert_eq!(frame.series(0), Some(&body[..16]));
        assert_eq!(frame.series(1), Some(&body[16..]));
    }

    #[test]
    fn builds_a_frame_from_whole_messages() {
        let (pool, mut frames) = (pool(), opened(false));
        let body: Vec<u8> = (0..32).collect();
        ends(&mut frames);
        assert!(draft(&mut frames, &pool).is_none());
        assert!(matches!(frames.decode(&body[..10]), Ok(Step::More)));
        let Ok(Step::Frame(frame)) = frames.decode(&body[10..]) else {
            panic!("a frame");
        };
        check(&frame, &body);
        assert!(frames.body().is_none());
    }

    #[test]
    fn builds_a_frame_from_bytes_received_in_place() {
        let (pool, mut frames) = (pool(), opened(false));
        let body: Vec<u8> = (100..132).collect();
        ends(&mut frames);
        assert!(draft(&mut frames, &pool).is_none());
        let rest = frames.body().expect("a body is due");
        assert_eq!(rest.len(), 32);
        rest[..20].copy_from_slice(&body[..20]);
        assert!(matches!(frames.received(20), Ok(None)));
        let rest = frames.body().expect("a body is due");
        assert_eq!(rest.len(), 12);
        rest.copy_from_slice(&body[20..]);
        let frame = frames.received(12).expect("decodes").expect("a frame");
        check(&frame, &body);
    }

    #[test]
    fn gives_a_frame_with_no_body_at_its_draft() {
        let (pool, mut frames) = (pool(), opened(false));
        frames.decode(&head(2)).expect("a head");
        let step = frames.decode(&encode_ends(&[(0, 0), (1, 0)]));
        assert!(matches!(step, Ok(Step::Draft)));
        let frame = draft(&mut frames, &pool).expect("a frame");
        assert_eq!(frame.series(0), Some(&[][..]));
        assert_eq!(frame.range(0), Some(Range { seq: 4, count: 2 }));
        assert!(frames.body().is_none());
        assert_eq!(frames.finished(), Error::Finished);
    }

    /// A wait for the pool that drops asks for the layout again.
    #[test]
    fn gives_the_layout_again() {
        let (pool, mut frames) = (pool(), opened(false));
        ends(&mut frames);
        let _first = frames.layout().expect("a layout");
        assert!(draft(&mut frames, &pool).is_none());
    }

    #[test]
    fn refuses_ends_that_break_the_layout() {
        let mut frames = opened(false);
        frames.decode(&head(1)).expect("a head");
        frames.decode(&encode_ends(&[(1, 8)])).expect("ends");
        assert_eq!(
            frames.layout().map(drop),
            Err(types::frame::Error::IndexAbsent { entry: 1, index: 0 })
        );
        let mut frames = opened(false);
        frames.decode(&head(2)).expect("a head");
        frames
            .decode(&encode_ends(&[(0, 16), (1, 8)]))
            .expect("ends");
        assert_eq!(
            frames.layout().map(drop),
            Err(types::frame::Error::End(BadEnd::Before {
                end: 8,
                start: 16
            }))
        );
    }

    #[test]
    fn refuses_a_body_message_past_the_body() {
        let (pool, mut frames) = (pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        frames.decode(&[0; 30]).expect("body");
        assert_eq!(
            frames.decode(&[0; 3]).map(drop),
            Err(Error::Body { len: 3, remain: 2 })
        );
    }

    #[test]
    fn gives_behind_and_the_error_of_each_finish() {
        let (pool, mut frames) = (pool(), opened(false));
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
        let (pool, mut frames) = (pool(), opened(false));
        ends(&mut frames);
        draft(&mut frames, &pool);
        draft(&mut frames, &pool);
    }

    #[test]
    #[should_panic(expected = "a body is due")]
    fn panics_at_bytes_received_with_no_body_due() {
        let _frame = opened(false).received(1);
    }
}
