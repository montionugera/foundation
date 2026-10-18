//! The parts of a frame after its head, in a read and in a write: the run of its ends,
//! then its body when the last end is not 0.

use std::sync::Arc;

use types::frame::key_set::KeySet;
use types::frame::{Draft, Layout};

use super::{Due, Error, ends};
use crate::common::{body, rest_of_run};

/// The part of a frame that a stream expects next.
#[derive(Clone, Copy, Debug)]
pub(super) enum Next {
    /// The head of the next frame.
    Head,
    /// A message of the run of ends, with the ends that remain in the run.
    Ends { remain: u32 },
    /// A message of the body.
    Body(body::Count),
}

impl Next {
    /// Decodes `message` as a message of the run. Gives the ends of the message, and
    /// whether the run ends with them.
    ///
    /// # Errors
    ///
    /// The errors of [`ends::decode`], then [`Error::Run`] for more ends than remain.
    /// The part stays as it was.
    ///
    /// # Panics
    ///
    /// When the next message is not of the run.
    #[inline]
    pub(super) fn ends<'m>(
        &mut self,
        message: &'m [u8],
    ) -> Result<(ends::Iter<'m>, bool), Error> {
        let Self::Ends { remain } = *self else {
            panic!("a run of ends is due, not {self:?}")
        };
        let ends = ends::decode(message)?;
        let remain = rest_of_run(remain, ends.len())?;
        *self = match (remain, ends.last_end()) {
            (0, Some(0)) => Self::Head,
            (0, Some(end)) => Self::Body(body::Count::new(to_usize(end))),
            _ => Self::Ends { remain },
        };
        Ok((ends, remain == 0))
    }

    /// Takes a body message of `len` bytes that the caller has in place, as
    /// [`Next::take`] takes it. Gives whether it is the last of the body.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `len` is 0.
    ///
    /// # Panics
    ///
    /// When the next message is not body bytes, or `len` is over the rest of the
    /// body.
    #[inline]
    pub(super) fn received(&mut self, len: usize) -> Result<bool, Error> {
        if let Self::Body(body) = *self {
            let remain = body.remain();
            assert!(
                len <= remain,
                "a body message of {len} bytes is over the {remain} bytes left of its \
                 body"
            );
        }
        self.take(len)
    }

    /// Takes a body message of `len` bytes. Gives whether it is the last of the body.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `len` is 0, and [`Error::Body`] when it is over the rest
    /// of the body. The part stays as it was.
    ///
    /// # Panics
    ///
    /// When the next message is not body bytes.
    #[inline]
    pub(super) fn take(&mut self, len: usize) -> Result<bool, Error> {
        let Self::Body(mut body) = *self else {
            panic!("a body is due, not {self:?}")
        };
        let last = body.take(len)?;
        *self = if last { Self::Head } else { Self::Body(body) };
        Ok(last)
    }

    /// Where in the body the next message starts, when the next message is body
    /// bytes.
    #[inline]
    pub(super) fn body(self) -> Option<usize> {
        match self {
            Self::Body(body) => Some(body.at()),
            Self::Head | Self::Ends { .. } => None,
        }
    }

    /// The bytes of the body that did not come, when the stream ends inside a body.
    pub(super) fn unfinished(self) -> Option<usize> {
        match self {
            Self::Body(body) => Some(body.remain()),
            Self::Head | Self::Ends { .. } => None,
        }
    }
}

/// The ends and the draft of the frame that arrives, from its run of ends to the last
/// byte of its body.
#[derive(Debug)]
pub(super) struct Assembly {
    set: Arc<KeySet>,
    /// The entry and end of each series.
    ends: Vec<(usize, usize)>,
    /// The ends are whole and the draft is not given.
    drafting: bool,
    /// `None` in the body of a skipped frame.
    draft: Option<Draft>,
}

impl Assembly {
    /// Makes room for one end per place, so no frame allocates.
    pub(super) fn new(set: Arc<KeySet>) -> Self {
        Self {
            ends: Vec::with_capacity(set.entries().len()),
            set,
            drafting: false,
            draft: None,
        }
    }

    pub(super) fn set(&self) -> &Arc<KeySet> {
        &self.set
    }

    /// What the frame waits for, when the next body message starts at `body`.
    #[inline]
    pub(super) fn due(
        &mut self,
        body: Option<usize>,
    ) -> Result<Due<'_>, types::frame::Error> {
        if self.drafting {
            return Layout::from_ends(&self.set, &self.ends).map(Due::Draft);
        }
        Ok(match (body, self.draft.as_mut()) {
            (Some(start), Some(draft)) => Due::Body(rest(draft, start)),
            _ => Due::Message,
        })
    }

    /// Starts the frame of a head.
    pub(super) fn head(&mut self) {
        self.ends.clear();
    }

    /// Takes the ends of a run message. The draft is due after the `last`.
    #[inline]
    pub(super) fn ends(&mut self, ends: ends::Iter<'_>, last: bool) {
        self.ends
            .extend(ends.map(|(place, end)| (to_usize(place), to_usize(end))));
        self.drafting = last;
    }

    /// Copies body bytes that start at `start` into the draft. A skipped frame drops
    /// them.
    #[inline]
    pub(super) fn body(&mut self, start: usize, bytes: &[u8]) {
        if let Some(draft) = self.draft.as_mut() {
            prefix(rest(draft, start), bytes.len()).copy_from_slice(bytes);
        }
    }

    /// Panics with `what` when a draft is due.
    #[inline]
    pub(super) fn check_undrafted(&self, what: &str) {
        assert!(!self.drafting, "a draft is due before {what}");
    }

    /// Panics unless body bytes are due into a draft.
    #[inline]
    pub(super) fn check_body(&self) {
        assert!(self.draft.is_some(), "a body is due");
    }

    /// Takes the draft that is due, or none for a skipped frame.
    ///
    /// # Panics
    ///
    /// When no draft is due.
    #[inline]
    pub(super) fn draft(&mut self, draft: Option<Draft>) {
        assert!(self.drafting, "no draft is due");
        self.drafting = false;
        self.draft = draft;
    }

    /// The draft of the frame that arrived, `None` for a skipped frame.
    #[inline]
    pub(super) fn take(&mut self) -> Option<Draft> {
        self.draft.take()
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

/// `n` as a `usize`.
pub(super) fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("invariant: a usize holds a u32")
}
