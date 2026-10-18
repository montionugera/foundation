use std::num::NonZeroUsize;
use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Form, Layout};

use super::{Error, Head, Time};
use crate::hub::ends;
use crate::hub::part::{Assembly, Next, to_usize};

/// The frames of a program's write stream after the places, at the node. The node
/// receives each message and makes each draft from the pool of the home. The entries
/// of each frame index the key set: entry `n` is place `n`. It holds 16 bytes for each
/// place, and allocates nothing after [`Frames::new`].
#[derive(Debug)]
pub struct Frames {
    places: u32,
    next: Next,
    head: Option<Head>,
    assembly: Assembly,
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
    /// A whole frame, with the count of its head set, for the home to write.
    Frame {
        /// The frame.
        draft: Draft,
        /// Where its stamps come from, and its path. For [`Time::Node`], the node
        /// writes the stamp over the one sample of the index before the write.
        time: Time,
    },
}

impl Frames {
    /// The frames of a write stream whose places are the entries of `set`, in order.
    ///
    /// # Panics
    ///
    /// When `set` does not have one group, or has more than `u32::MAX` entries.
    #[must_use]
    pub fn new(set: Arc<KeySet>) -> Self {
        assert_eq!(set.groups().len(), 1, "a write stream is on one index");
        let places = u32::try_from(set.entries().len())
            .expect("a write stream has at most u32::MAX places");
        Self {
            places,
            next: Next::Head,
            head: None,
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
    /// [`Error::Form`], [`Error::Samples`], [`Error::Series`], [`Error::Stamp`], and
    /// [`Error::Places`] for more series than places. For a message of the run of
    /// ends, the errors of its bytes, then [`Error::Run`]. For a body message,
    /// [`Error::Empty`] and [`Error::Body`]. The frames stay as they were. The caller
    /// then stops the stream as `MALFORMED`.
    ///
    /// # Panics
    ///
    /// When [`Frames::due`] gives [`Due::Draft`].
    #[inline]
    pub fn decode(&mut self, message: &[u8]) -> Result<Step, Error> {
        self.assembly.check_undrafted("the next message");
        match self.next {
            Next::Head => self.head(message),
            Next::Ends { .. } => {
                let (ends, last) = self.next.ends(message)?;
                self.assembly.ends(ends, last);
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
            Next::Head => return Ok(()),
            Next::Ends { remain } => (remain, None),
            Next::Body(body) => (0, Some(body)),
        };
        let series = self.head.expect("invariant: a frame has a head").series;
        let ends = series
            .checked_sub(remain)
            .expect("invariant: the ends that remain are of the head's series");
        let fit = "invariant: the bytes of a frame fit a usize";
        let sum = |a: usize, b: usize| a.checked_add(b).expect(fit);
        let run = sum(Head::LEN, to_usize(ends).checked_mul(ends::LEN).expect(fit));
        Err(match body {
            None => Error::Cut {
                received: run,
                len: None,
            },
            Some(body) => Error::Cut {
                received: sum(run, body.at()),
                len: Some(
                    NonZeroUsize::new(sum(sum(run, body.at()), body.remain()))
                        .expect("invariant: a frame holds its head"),
                ),
            },
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
        self.assembly.head();
        self.next = Next::Ends {
            remain: head.series,
        };
        Ok(Step::Head(head))
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
        draft.set_count(0, head.count);
        Step::Frame {
            draft,
            time: head.time,
        }
    }
}

#[cfg(test)]
mod tests;
