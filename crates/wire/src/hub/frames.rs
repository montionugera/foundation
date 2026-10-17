use std::ops;
use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Frame, Layout};

use super::{Error, FromHome, Gaps, Head, Reader};

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
    /// `None` in a latest session.
    gaps: Option<Gaps>,
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
            return Err(reader.finished());
        };
        match reader.decode(message)? {
            FromHome::Opened { live } => {
                let frames = Self {
                    gaps: (!reader.latest()).then(|| Gaps::new(live)),
                    reader,
                    set,
                    head: None,
                    ends: Vec::new(),
                    drafting: false,
                    draft: None,
                };
                Ok((frames, live))
            }
            other => {
                panic!("a reader before opened gave {other:?} for its first message")
            }
        }
    }

    /// The key set of each frame.
    #[must_use]
    pub fn set(&self) -> &Arc<KeySet> {
        &self.set
    }

    /// The seqs of the index lost just before the last frame that
    /// [`Frames::decode`], [`Frames::received`], or [`Frames::draft`] gave: from the
    /// end of the last frame with a sample, or the open position, to the frame's first
    /// seq. `0..0` when none, before the first frame, for a frame with no sample, and
    /// in a latest session.
    #[must_use]
    #[inline]
    pub fn lost(&self) -> ops::Range<u64> {
        self.gaps.as_ref().map_or(0..0, Gaps::last)
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
            other @ (FromHome::Opened { .. } | FromHome::Body { .. }) => {
                unreachable!("invariant: an open decoder outside a body gave {other:?}")
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

    /// The error of a stream that the home finished at this point. The home finishes
    /// its stream only after the reader finished its own.
    #[must_use]
    pub fn finished(&self) -> Error {
        self.reader.finished()
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
        if let Some(gaps) = &mut self.gaps {
            gaps.pass(head.range);
        }
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
    use crate::hub::{HEAD, Mode, OPENED, Open, Reply};

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
        assert_eq!(open(Some(&[3])), Err(Error::Kind { kind: 3 }));
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
    #[should_panic(expected = "a reader before opened gave")]
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
    fn gives_the_error_of_each_finish() {
        let (pool, mut frames) = (create_pool(), opened(false));
        assert_eq!(frames.finished(), Error::Finished);
        ends(&mut frames);
        draft(&mut frames, &pool);
        frames.decode(&[0; 30]).expect("body");
        assert_eq!(frames.finished(), Error::Unfinished { remain: 2 });
        frames.decode(&[0; 2]).expect("body");
        assert_eq!(frames.finished(), Error::Finished);
        for latest in [false, true] {
            let mut frames = opened(latest);
            assert_eq!(frames.decode(&[3]).map(drop), Err(Error::Kind { kind: 3 }));
        }
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

    mod lost {
        use proptest::prelude::*;

        use super::*;

        /// The frames of a session of 2 places opened at `live`, latest when `latest`.
        fn opened_at_live(latest: bool, live: u64) -> Frames {
            let opened = opened_at(live);
            let (frames, _) =
                Frames::open(create_reader(latest), create_set(), Some(&opened))
                    .expect("opened");
            frames
        }

        /// Gives `frames` a frame of `count` samples from `seq` with no body, and
        /// gives what `lost` is then.
        fn lost_at(frames: &mut Frames, seq: u64, count: u32) -> ops::Range<u64> {
            frames.decode(&head(seq, count, 1)).expect("a head");
            frames.decode(&encode_ends(&[(0, 0)])).expect("ends");
            let Step::Frame(frame) = draft(frames, &create_pool()) else {
                panic!("a frame");
            };
            assert_eq!(frame.range(0), Some(Range { seq, count }));
            frames.lost()
        }

        #[test]
        fn is_empty_before_the_first_frame() {
            assert_eq!(opened_at_live(false, 5).lost(), 0..0);
        }

        #[test]
        fn gives_the_seqs_from_the_open_position_to_the_first_frame() {
            let mut frames = opened_at_live(false, 5);
            assert_eq!(lost_at(&mut frames, 9, 2), 5..9);
            assert_eq!(lost_at(&mut frames, 11, 1), 0..0);
        }

        #[test]
        fn gives_no_gap_to_a_first_frame_at_the_open_position() {
            let mut frames = opened_at_live(false, 5);
            assert_eq!(lost_at(&mut frames, 5, 2), 0..0);
        }

        #[test]
        fn gives_a_jump_after_a_frame() {
            let mut frames = opened_at_live(false, 0);
            assert_eq!(lost_at(&mut frames, 0, 3), 0..0);
            assert_eq!(lost_at(&mut frames, 4, 1), 3..4);
        }

        /// A frame with no sample gives no gap and keeps its start, so the next
        /// frame with a sample gives the whole gap.
        #[test]
        fn gives_the_gap_of_a_frame_with_no_sample_at_the_next_frame() {
            let mut frames = opened_at_live(false, 2);
            assert_eq!(lost_at(&mut frames, 6, 0), 0..0);
            assert_eq!(lost_at(&mut frames, 8, 1), 2..8);
        }

        #[test]
        fn gives_no_gap_in_a_latest_session() {
            let mut frames = opened_at_live(true, 5);
            assert_eq!(lost_at(&mut frames, 9, 2), 0..0);
            assert_eq!(lost_at(&mut frames, 20, 1), 0..0);
        }

        /// A random `live` and the seq and count of each frame of a session from
        /// it, of random jumps and counts, 0 too.
        fn jumps() -> impl Strategy<Value = (u64, Vec<(u64, u32)>)> {
            let jumps = prop::collection::vec((0..4u64, 0..4u32), 1..32);
            (0..1_000u64, jumps).prop_map(|(live, jumps)| {
                let mut end = live;
                let frames = jumps.into_iter().map(|(jump, count)| {
                    let seq = end.checked_add(jump).expect("small");
                    end = seq.checked_add(u64::from(count)).expect("small");
                    (seq, count)
                });
                (live, frames.collect())
            })
        }

        proptest! {
            /// Each gap and the range of each frame with a sample tile the seqs from
            /// `live` to the end of the last such frame, with no overlap and no hole.
            #[test]
            fn tiles_the_seqs_with_the_frames((live, sent) in jumps()) {
                let mut frames = opened_at_live(false, live);
                let mut covered = live;
                for (seq, count) in sent {
                    let lost = lost_at(&mut frames, seq, count);
                    if count == 0 {
                        prop_assert_eq!(lost, 0..0);
                        continue;
                    }
                    if !lost.is_empty() {
                        prop_assert_eq!(lost.start, covered);
                        covered = lost.end;
                    }
                    prop_assert_eq!(covered, seq);
                    covered = seq.checked_add(u64::from(count)).expect("small");
                }
            }

            #[test]
            fn gives_no_gap_at_any_frame_of_a_latest_session(
                (live, sent) in jumps(),
            ) {
                let mut frames = opened_at_live(true, live);
                for (seq, count) in sent {
                    prop_assert_eq!(lost_at(&mut frames, seq, count), 0..0);
                }
            }
        }
    }
}
