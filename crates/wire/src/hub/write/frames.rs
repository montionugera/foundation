use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Form, Layout};

use super::{Error, Head, Time};
use crate::hub::part::{Next, Part, prefix, rest, to_usize};

/// The frames of a program's write stream after the places, at the node. The node
/// receives each message and makes each draft from the pool of the home. The entries
/// of each frame index the key set: entry `n` is place `n`.
#[derive(Debug)]
pub struct Frames {
    set: Arc<KeySet>,
    places: u32,
    next: Next,
    head: Option<Head>,
    /// The entry and end of each series of the frame that arrives.
    ends: Vec<(usize, usize)>,
    /// The form of the draft that is due: the ends are whole and the draft is not
    /// given.
    drafting: Option<Form>,
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
            set,
            places,
            next: Next::Head,
            head: None,
            ends: Vec::new(),
            drafting: None,
            draft: None,
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
        if let Some(form) = self.drafting {
            return Layout::from_ends(&self.set, &self.ends)
                .map(|layout| Due::Draft { layout, form });
        }
        Ok(match (self.next.body(), self.draft.as_mut()) {
            (Some(start), Some(draft)) => Due::Body(rest(draft, start)),
            (None, _) => Due::Message,
            (Some(_), None) => unreachable!("invariant: a body has a draft"),
        })
    }

    /// Takes the next message, whole. A body message is copied into the draft. When
    /// the program finishes its stream inside a frame, drop the frames: the program
    /// has no outcome for that frame.
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
        assert!(
            self.drafting.is_none(),
            "a draft is due before the next message"
        );
        let start = self.next.body();
        if let Next::Head = self.next {
            return self.head(message);
        }
        match self.next.decode(message)? {
            Part::Ends { ends, last } => {
                self.ends
                    .extend(ends.map(|(place, end)| (to_usize(place), to_usize(end))));
                if last {
                    self.drafting =
                        Some(self.head.expect("invariant: a run has a head").form);
                }
                Ok(Step::More)
            }
            Part::Body { bytes, last } => {
                let start = start.expect("invariant: body bytes come inside a body");
                let draft = self.draft.as_mut().expect("invariant: a body has a draft");
                prefix(rest(draft, start), bytes.len()).copy_from_slice(bytes);
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
        assert!(self.drafting.is_none(), "a body is due");
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
        assert!(self.drafting.take().is_some(), "no draft is due");
        self.draft = Some(draft);
        match self.next.body() {
            Some(_) => Step::More,
            None => self.frame(),
        }
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
        self.ends.clear();
        self.next = Next::Ends {
            remain: head.series,
        };
        Ok(Step::Head(head))
    }

    /// The step of the body message that arrived, a frame when it is the `last`.
    fn received_body(&mut self, last: bool) -> Step {
        if last { self.frame() } else { Step::More }
    }

    /// The frame of the head and the draft that arrived.
    fn frame(&mut self) -> Step {
        let head = self.head.take().expect("invariant: a frame has a head");
        let mut draft = self.draft.take().expect("invariant: a frame has a draft");
        draft.set_count(0, head.count);
        Step::Frame {
            draft,
            time: head.time,
        }
    }
}

#[cfg(test)]
mod tests;
