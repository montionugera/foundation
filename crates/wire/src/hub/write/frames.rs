use std::num::NonZeroUsize;
use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Form, Layout};

use super::{Error, Head, Time, counts};
use crate::common::rest_of_run;
use crate::hub::part::{Assembly, Next, to_usize};
use crate::hub::{ends, run};

/// The frames of a program's write stream after the places, at the node. The node
/// receives each message and makes each draft from the pool of the home. The entries
/// of each frame index the key set: entry `n` is place `n`. It holds
/// [`Frames::bytes`] for its places, and allocates nothing after [`Frames::new`].
#[derive(Debug)]
pub struct Frames {
    places: u32,
    next: Next,
    head: Option<Head>,
    /// Each present group of the frame and its count, once its ends are whole.
    counts: Vec<(u32, u32)>,
    /// The counts that came. The run of counts is due while it is below
    /// `counts.len()`.
    counted: usize,
    assembly: Assembly<true>,
}

/// What [`Frames`] waits for next.
#[derive(Debug)]
pub enum Due<'f> {
    /// A whole message, for [`Frames::decode`]. The body of a skipped frame comes so.
    Message,
    /// Body bytes. Receive into the slice, the rest of the draft's body, and give the
    /// length to [`Frames::received`]. A message over the slice goes to
    /// [`Frames::decode`] whole, which gives its error.
    Body(&'f mut [u8]),
    /// A draft of this layout and form from the pool of the home, for
    /// [`Frames::draft`].
    Draft {
        /// The layout of the frame's ends.
        layout: Layout<'f>,
        /// The form of the head.
        form: Form,
    },
}

/// What one input gave.
#[derive(Debug)]
pub enum Step {
    /// The head of a frame. The run of its ends follows.
    Head(Head),
    /// Part of a run of ends or of a body, or a draft that the body fills.
    More,
    /// A whole frame, with the count of each present group set, for the home to
    /// write.
    Frame {
        /// The frame.
        draft: Draft,
        /// Where its stamps come from, and its path. For [`Time::Node`], the node
        /// writes the stamp over the one sample of each index before the write.
        time: Time,
    },
}

impl Frames {
    /// The bytes that a `Frames` of `places` places holds for their ends and counts.
    #[must_use]
    pub const fn bytes(places: usize) -> usize {
        Assembly::<true>::bytes(places)
            .saturating_add(places.saturating_mul(size_of::<(u32, u32)>()))
    }

    /// The frames of a write stream whose places are the entries of `set`, in order.
    ///
    /// # Panics
    ///
    /// When `set` has more than `u32::MAX` entries.
    #[must_use]
    pub fn new(set: Arc<KeySet>) -> Self {
        let places = u32::try_from(set.entries().len())
            .expect("a write stream has at most u32::MAX places");
        Self {
            places,
            next: Next::Head,
            head: None,
            counts: Vec::with_capacity(set.entries().len()),
            counted: 0,
            assembly: Assembly::reserved(set),
        }
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
        if self.assembly.drafting() {
            let Some(Head { form, .. }) = self.head else {
                unreachable!("invariant: a draft has a head")
            };
            return self
                .assembly
                .layout()
                .map(|layout| Due::Draft { layout, form });
        }
        Ok(self
            .assembly
            .rest(self.next.body())
            .map_or(Due::Message, Due::Body))
    }

    /// Takes the next message, whole. A body message is copied into the draft, or
    /// dropped for a skipped frame. When the program finishes its stream, call
    /// [`Frames::finish`].
    ///
    /// # Errors
    ///
    /// For a head, in order: [`Error::Empty`], [`Error::Kind`] for a kind other than
    /// the head, [`Error::Length`], [`Error::Path`] for a time that no head has,
    /// [`Error::Form`], [`Error::Series`], [`Error::Stamp`], and [`Error::Places`] for
    /// more series than places. For a message of the run of ends, the errors of its
    /// bytes, then [`Error::Run`]. For a message of the run of counts, the errors of
    /// its bytes, then [`Error::Run`] and [`Error::Samples`]. For a body message,
    /// [`Error::Empty`] and [`Error::Body`]. The frames stay as they were. The caller
    /// then stops the stream as `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] gives [`Due::Draft`].
    #[inline]
    pub fn decode(&mut self, message: &[u8]) -> Result<Step, Error> {
        self.assembly.check_undrafted("the next message");
        if self.counted < self.counts.len() {
            return self.counts(message);
        }
        match self.next {
            Next::Head => self.head(message),
            Next::Ends { .. } => {
                let (ends, last) = self.next.ends(message)?;
                self.assembly.ends(ends, false);
                if last {
                    self.ended();
                }
                Ok(Step::More)
            }
            Next::Body(body) => {
                let last = self.next.take(message.len())?;
                self.assembly.body(body.at(), message);
                Ok(self.received_body(last))
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
        self.assembly.check_body();
        let last = self.next.received(len)?;
        Ok(self.received_body(last))
    }

    /// Takes the draft, which is of the layout and form of [`Due::Draft`]. Gives the
    /// frame when it has no body, else [`Step::More`].
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] does not give [`Due::Draft`].
    #[inline]
    pub fn draft(&mut self, draft: Draft) -> Step {
        self.assembly.draft(draft);
        match self.next.body() {
            Some(_) => Step::More,
            None => self.frame(),
        }
    }

    /// Takes no draft for the frame whose draft is due, when the pool of the home has
    /// no block for it. The frame's body, when it has one, then comes as
    /// [`Due::Message`], and [`Frames::decode`] checks each message, drops it, and
    /// gives [`Step::More`].
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] does not give [`Due::Draft`].
    #[inline]
    pub fn skip(&mut self) {
        self.assembly.skip();
        if self.next.body().is_none() {
            self.frame();
        }
    }

    /// Checks the end of the stream, when the program finished it.
    ///
    /// # Errors
    ///
    /// [`Error::Cut`] inside a frame. The caller then stops the stream as `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] gives [`Due::Draft`].
    pub fn finish(&self) -> Result<(), Error> {
        self.assembly.check_undrafted("the end of the stream");
        let (remain, body) = match self.next {
            Next::Head if self.counted == self.counts.len() => return Ok(()),
            Next::Head => (0, None),
            Next::Ends { remain } => (remain, None),
            Next::Body(body) => (0, Some(body)),
        };
        let head = self.head.expect("invariant: a frame has a head");
        let ends = head
            .series
            .checked_sub(remain)
            .expect("invariant: the ends that remain are of the head's series");
        let fit = "invariant: the bytes of a frame fit a usize";
        let sum = |a: usize, b: usize| a.checked_add(b).expect(fit);
        let product = |a: usize, b: usize| a.checked_mul(b).expect(fit);
        let run = sum(Head::LEN, product(to_usize(ends), ends::LEN));
        if remain > 0 {
            return Err(Error::Cut {
                received: run,
                len: None,
            });
        }
        let (at, len) =
            body.map_or((0, 0), |body| (body.at(), sum(body.at(), body.remain())));
        // A head that the node stamps has no run of counts.
        let counts = |n| match head.time {
            Time::Frame(_) => product(n, counts::LEN),
            Time::Node => 0,
        };
        let counted = sum(run, counts(self.counted));
        let whole = sum(run, counts(self.counts.len()));
        Err(Error::Cut {
            received: sum(counted, at),
            len: Some(
                NonZeroUsize::new(sum(whole, len))
                    .expect("invariant: a frame holds its head"),
            ),
        })
    }

    fn head(&mut self, message: &[u8]) -> Result<Step, Error> {
        let head = Head::decode(message)?;
        if head.series > self.places {
            return Err(Error::Places {
                series: head.series,
                places: self.places,
            });
        }
        self.head = Some(head);
        self.counts.clear();
        self.counted = 0;
        self.assembly.head();
        self.next = Next::Ends {
            remain: head.series,
        };
        Ok(Step::Head(head))
    }

    /// Starts the run of counts after the last end: one count for each index among the
    /// ends. A head of [`Time::Node`] has none, and each of its groups holds 1.
    fn ended(&mut self) {
        let set = self.assembly.set();
        let entries = set.entries();
        // Only an index above the last one counts, so that a group whose index repeats
        // asks one count. The layout refuses such ends.
        let mut last = None;
        self.counts
            .extend(self.assembly.entries().filter_map(|entry| {
                let place = entries.get(entry)?;
                let index =
                    set.index(entry) == entry && last.is_none_or(|last| entry > last);
                index.then(|| {
                    last = Some(entry);
                    (place.group, 1)
                })
            }));
        if self.head.is_some_and(|head| head.time == Time::Node) {
            self.counted = self.counts.len();
        }
        if self.counted == self.counts.len() {
            self.assembly.whole();
        }
    }

    /// Takes a message of the run of counts.
    fn counts(&mut self, message: &[u8]) -> Result<Step, Error> {
        let counts = run::<{ counts::LEN }>(message)?;
        rest_of_run(self.uncounted(), counts.len())?;
        if counts.contains(&[0; counts::LEN]) {
            return Err(Error::Samples);
        }
        self.fill(counts);
        Ok(Step::More)
    }

    /// The counts of the run that did not come.
    fn uncounted(&self) -> u32 {
        let remain = self
            .counts
            .len()
            .checked_sub(self.counted)
            .expect("invariant: no more counts came than are due");
        u32::try_from(remain).expect("invariant: a frame has at most u32::MAX groups")
    }

    /// Sets the next counts that are due from `counts`, which do not pass the run.
    fn fill(&mut self, counts: &[[u8; counts::LEN]]) {
        let due = self
            .counts
            .get_mut(self.counted..)
            .expect("invariant: no more counts came than are due");
        for ((_, count), &bytes) in due.iter_mut().zip(counts) {
            *count = u32::from_le_bytes(bytes);
        }
        self.counted = self
            .counted
            .checked_add(counts.len())
            .expect("invariant: the counts fit a usize");
        if self.counted == self.counts.len() {
            self.assembly.whole();
        }
    }

    /// The step of the body message that arrived, the frame when it is the `last`.
    fn received_body(&mut self, last: bool) -> Step {
        if last { self.frame() } else { Step::More }
    }

    /// The frame of the head and the draft that arrived, or [`Step::More`] when the
    /// frame was skipped.
    #[inline]
    fn frame(&mut self) -> Step {
        let head = self.head.take().expect("invariant: a frame has a head");
        let Some(mut draft) = self.assembly.take() else {
            return Step::More;
        };
        for &(group, count) in &self.counts {
            draft.set_count(group, count);
        }
        Step::Frame {
            draft,
            time: head.time,
        }
    }
}

#[cfg(test)]
mod tests;
