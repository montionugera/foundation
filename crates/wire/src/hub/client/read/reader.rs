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
    Before(Before),
    Home(hub::Reader),
}

/// The message the stream expects before the last place.
#[derive(Debug)]
enum Before {
    Places,
    Run { places: Places, remain: u32 },
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
            next: Next::Before(Before::Places),
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
            Next::Home(reader) => reader.decode(message).map(FromNode::Home),
            Next::Before(before) => {
                let (event, home) = before.decode(message, self.latest)?;
                if let Some(home) = home {
                    self.next = Next::Home(home);
                }
                Ok(event)
            }
        }
    }

    /// As [`hub::Reader::body`](crate::hub::Reader::body), and `None` before the
    /// last place.
    #[must_use]
    pub fn body(&self) -> Option<usize> {
        match &self.next {
            Next::Home(reader) => reader.body(),
            Next::Before(_) => None,
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
            Next::Before(_) => Err(Error::Finished),
        }
    }
}

impl Before {
    /// Decodes `message`. After the last place, it also gives the session, latest
    /// when `latest`.
    #[cold]
    #[inline(never)]
    fn decode<'m>(
        &mut self,
        message: &'m [u8],
        latest: bool,
    ) -> Result<(FromNode<'m>, Option<hub::Reader>), Error> {
        match self {
            Self::Places => {
                let places = Places::decode(message)?;
                *self = Self::Run {
                    places,
                    remain: places.count,
                };
                Ok((FromNode::Places(places), None))
            }
            Self::Run { places, remain } => {
                let run = place::decode(message)?;
                let rest = rest_of_run(*remain, run.len())?;
                let first = places.count.checked_sub(*remain).unwrap_or_else(|| {
                    panic!(
                        "invariant: remain {remain} is at most count {}",
                        places.count
                    )
                });
                let index = (places.index.checked_sub(first))
                    .and_then(|at| run.get(usize::try_from(at).ok()?));
                if let Some(index) = index
                    && index.data_type != Type::Scalar(Scalar::Stamp)
                {
                    return Err(Error::Unstamped {
                        sample: index.data_type,
                    });
                }
                *remain = rest;
                let home = (rest == 0).then(|| hub::Reader::of(places.count, latest));
                Ok((FromNode::Run(run), home))
            }
        }
    }
}
