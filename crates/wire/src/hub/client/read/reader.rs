use types::sample::{Scalar, Type};

use super::{Mode, Place, Places, place};
use crate::common::rest_of_run;
use crate::hub::{self, client::Error};

/// A decoder of the start of a read stream at the program: the places and their
/// run. With the last place it gives the decoder of the replies that follow.
///
/// The stream may not end before the last place. When the node finishes it there,
/// the caller gives [`Error::Finished`].
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
    Given,
}

/// One message of the start of a read stream from the node.
#[derive(Debug)]
pub enum FromNode {
    /// The first message. The run of its places follows.
    Places(Places),
    /// One message of the run of places, in place order.
    Run {
        /// The places in the message.
        places: Vec<Place>,
        /// With the last place, the decoder of the replies of the reader session,
        /// for the count of places and the mode of the read. Else `None`.
        home: Option<hub::Reader>,
    },
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

    /// Decodes the next message from the node, up to the last place.
    ///
    /// # Errors
    ///
    /// The first message decodes as [`Places`] and gives its errors:
    /// [`Error::Empty`], [`Error::Kind`] for another kind, [`Error::Length`], or
    /// [`Error::Index`]. A message of the run gives, in order, the error of its bytes
    /// ([`Error::Empty`], [`Error::Length`], [`Error::Sample`], or [`Error::Name`]),
    /// [`Error::Run`] when it holds more places than remain, and
    /// [`Error::Unstamped`] when it holds the index place and that place's sample
    /// type is not `Stamp`.
    ///
    /// The stream is then not valid ([`MALFORMED`](crate::header::MALFORMED)), and
    /// the caller stops it.
    ///
    /// # Panics
    ///
    /// When a call follows the message that gave `home`.
    pub fn decode(&mut self, message: &[u8]) -> Result<FromNode, Error> {
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
                let home =
                    (rest == 0).then(|| hub::Reader::of(places.count, self.latest));
                if home.is_some() {
                    self.next = Next::Given;
                }
                Ok(FromNode::Run { places: run, home })
            }
            Next::Given => {
                panic!("a read stream is decoded after the decoder gave its home")
            }
        }
    }
}
