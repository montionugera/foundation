//! The parts of a frame after its head, in a read and in a write: the run of its ends,
//! then its body when the last end is not 0.

use types::frame::Draft;

use super::{Error, ends};
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

/// A message of a run of ends or of a body, decoded.
pub(super) enum Part<'m> {
    /// The ends of the message, each a place and an end. The run ends with the `last`.
    Ends { ends: ends::Iter<'m>, last: bool },
    /// The bytes of the message. The body ends with the `last`.
    Body { bytes: &'m [u8], last: bool },
}

impl Next {
    /// Decodes `message` as the next message of the run of ends or of the body.
    ///
    /// # Errors
    ///
    /// For a message of the run, the errors of [`ends::decode`], then [`Error::Run`]
    /// for more ends than remain. For a body message, [`Error::Empty`] and
    /// [`Error::Body`]. The part stays as it was.
    ///
    /// # Panics
    ///
    /// When a head is due.
    #[inline]
    pub(super) fn decode<'m>(&mut self, message: &'m [u8]) -> Result<Part<'m>, Error> {
        match *self {
            Self::Ends { remain } => {
                let ends = ends::decode(message)?;
                let remain = rest_of_run(remain, ends.len())?;
                *self = match (remain, ends.last_end()) {
                    (0, Some(0)) => Self::Head,
                    (0, Some(end)) => Self::Body(body::Count::new(to_usize(end))),
                    _ => Self::Ends { remain },
                };
                Ok(Part::Ends {
                    ends,
                    last: remain == 0,
                })
            }
            Self::Body(body) => Ok(Part::Body {
                bytes: message,
                last: self.take(body, message.len())?,
            }),
            Self::Head => panic!("a head is due, not a part of a frame"),
        }
    }

    /// Takes a body message of `len` bytes that the caller has in place, as
    /// [`Next::decode`] takes it. Gives whether it is the last of the body.
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
        let Self::Body(body) = *self else {
            panic!("a body is due, not {self:?}")
        };
        let remain = body.remain();
        assert!(
            len <= remain,
            "a body message of {len} bytes is over the {remain} bytes left of its body"
        );
        self.take(body, len)
    }

    fn take(&mut self, mut body: body::Count, len: usize) -> Result<bool, Error> {
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

/// The body of `draft` from `start` on.
pub(super) fn rest(draft: &mut Draft, start: usize) -> &mut [u8] {
    let body = draft.body_mut();
    let len = body.len();
    body.get_mut(start..).unwrap_or_else(|| {
        panic!(
            "invariant: a body that starts at {start} is inside its {len} byte draft"
        )
    })
}

/// The first `len` bytes of `rest`.
pub(super) fn prefix(rest: &mut [u8], len: usize) -> &mut [u8] {
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
