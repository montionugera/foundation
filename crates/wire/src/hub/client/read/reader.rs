use types::sample::{Scalar, Type};

use super::{Mode, Place, Places, place};
use crate::hub::{self, Error, FromHome, rest_of_run};

/// A decoder of a read stream at the program: the places, their run, and then the
/// replies of the reader session at the home.
#[derive(Debug)]
pub struct Reader {
    latest: bool,
    next: Next,
}

/// The message the stream expects next.
#[derive(Debug)]
enum Next {
    Places,
    Run { places: Places, remain: u32 },
    Home(hub::Reader),
}

/// One message of a read stream from the node.
#[derive(Clone, Debug)]
pub enum FromNode<'m> {
    /// The first reply. The run of its places follows.
    Places(Places),
    /// One message of the run of places, in place order.
    Run(Vec<Place>),
    /// One reply of the reader session, after the last place.
    Home(FromHome<'m>),
}

impl Reader {
    /// A decoder for a read that `mode` opened.
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        Self {
            latest: mode == Mode::Latest,
            next: Next::Places,
        }
    }

    /// Decodes the next message from the node.
    ///
    /// # Errors
    ///
    /// The first message decodes as [`Places`], so another kind gives its error:
    /// [`Error::Empty`], [`Error::Kind`], [`Error::Length`], or [`Error::Index`]. A
    /// message of the run gives, in order, the error of its bytes ([`Error::Empty`],
    /// [`Error::Length`], [`Error::Sample`], or [`Error::Name`]), [`Error::Run`] when
    /// it holds more places than remain, and [`Error::Unstamped`] when it holds the
    /// index place and that place's sample type is not `Stamp`. After the last place,
    /// the errors of [`hub::Reader::decode`].
    ///
    /// The stream is then not valid ([`MALFORMED`](crate::header::MALFORMED)), and
    /// the caller stops it.
    pub fn decode<'m>(&mut self, message: &'m [u8]) -> Result<FromNode<'m>, Error> {
        match &mut self.next {
            Next::Places => {
                let places = Places::decode(message)?;
                self.next = Next::Run {
                    places,
                    remain: places.count,
                };
                Ok(FromNode::Places(places))
            }
            Next::Run { places, remain } => {
                let run = place::decode(message)?;
                let rest = rest_of_run(*remain, run.len())?;
                // `remain` is at most `count`.
                let first = places.count.wrapping_sub(*remain);
                let at = places.index.checked_sub(first);
                stamped(at.and_then(|at| run.get(usize::try_from(at).ok()?)))?;
                *remain = rest;
                if rest == 0 {
                    self.next = Next::Home(hub::Reader::of(places.count, self.latest));
                }
                Ok(FromNode::Run(run))
            }
            Next::Home(reader) => reader.decode(message).map(FromNode::Home),
        }
    }

    /// As [`hub::Reader::body`](crate::hub::Reader::body), and `None` before the
    /// last place.
    #[must_use]
    pub fn body(&self) -> Option<usize> {
        match &self.next {
            Next::Home(reader) => reader.body(),
            Next::Places | Next::Run { .. } => None,
        }
    }

    /// As [`hub::Reader::end`](crate::hub::Reader::end), and [`Error::Finished`]
    /// before the last place.
    ///
    /// # Errors
    ///
    /// [`Error::Finished`] before the last place, and the errors of
    /// [`hub::Reader::end`] after it.
    pub fn end(&self) -> Result<(), Error> {
        match &self.next {
            Next::Home(reader) => reader.end(),
            Next::Places | Next::Run { .. } => Err(Error::Finished),
        }
    }
}

/// Checks `index`, the index place when the message holds it.
fn stamped(index: Option<&Place>) -> Result<(), Error> {
    match index {
        Some(place) if place.data_type != Type::Scalar(Scalar::Stamp) => {
            Err(Error::Unstamped {
                sample: place.data_type,
            })
        }
        Some(_) | None => Ok(()),
    }
}
