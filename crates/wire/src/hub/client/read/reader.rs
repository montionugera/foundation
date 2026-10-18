use std::sync::Arc;

use types::frame::key_set::KeySet;

use super::{Mode, Place, Places};
use crate::hub::client::run::Run;
use crate::hub::{self, client::Error};

/// A decoder of the start of a read stream at the program: the places and their
/// run. With the last place it gives the [`Start`] of the read.
#[derive(Debug)]
pub struct Reader {
    latest: bool,
    next: Next,
}

/// The message the stream expects next.
#[derive(Debug)]
enum Next {
    Places,
    Run(Run),
    Given,
}

/// The start of a read: its places, their key set, and the decoder of the replies.
/// Only [`Reader::decode`] makes one that holds together, and no call takes one.
#[derive(Debug)]
pub struct Start {
    /// The places, in place order.
    pub places: Vec<Place>,
    /// The key set of each frame of the read, from [`KeySet::ordered`]: entry `n` is
    /// place `n`.
    pub set: Arc<KeySet>,
    /// The decoder of the replies of the reader session, for the count of places and
    /// the mode of the read.
    pub home: hub::Reader,
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

    /// Decodes the next message from the node, up to the last place, or `None` when
    /// the node finished the stream. Gives the start of the read with the last place,
    /// else `None`.
    ///
    /// # Errors
    ///
    /// [`Error::Finished`] when `message` is `None`. The first message decodes as
    /// [`Places`] and gives its errors: [`Error::Empty`], [`Error::Kind`] for another
    /// kind, [`Error::Length`], [`Error::Places`], or [`Error::Index`]. A message of
    /// the run gives, in order, the error of its bytes ([`Error::Empty`],
    /// [`Error::Length`], [`Error::Sample`], or [`Error::Name`]), [`Error::Run`] when
    /// it holds more places than remain, and [`Error::Unstamped`] when it holds the
    /// index place and that place's sample type is not `Stamp`. The last message then
    /// gives [`Error::Repeated`] when the places name a channel more than once.
    ///
    /// The stream is then not valid ([`MALFORMED`](crate::header::MALFORMED)), and
    /// the caller stops it. A refused message leaves the decoder as it was.
    ///
    /// # Panics
    ///
    /// When it gave the start before.
    pub fn decode(&mut self, message: Option<&[u8]>) -> Result<Option<Start>, Error> {
        match (&mut self.next, message) {
            (Next::Given, _) => {
                panic!("a read stream is decoded after the decoder gave its start")
            }
            (_, None) => Err(Error::Finished),
            (Next::Places, Some(message)) => {
                self.next = Next::Run(Run::new(Places::decode(message)?));
                Ok(None)
            }
            (Next::Run(run), Some(message)) => {
                let count = run.count();
                let Some((places, set)) = run.decode(message)? else {
                    return Ok(None);
                };
                self.next = Next::Given;
                Ok(Some(Start {
                    places,
                    set: Arc::new(set),
                    home: hub::Reader::of(count, self.latest),
                }))
            }
        }
    }
}
