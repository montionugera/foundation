//! The run of places after a [`Places`], as the program decodes it.

use types::frame::key_set::KeySet;
use types::sample::{Scalar, Type};

use super::{Error, Place, Places, place};
use crate::common::rest_of_run;

/// The run of a [`Places`], with the places that arrived.
#[derive(Debug)]
pub(super) struct Run {
    places: Places,
    remain: u32,
    arrived: Vec<Place>,
}

impl Run {
    pub(super) fn new(places: Places) -> Self {
        Self {
            places,
            remain: places.count,
            arrived: Vec::new(),
        }
    }

    /// The count of places, the index too.
    pub(super) fn count(&self) -> u32 {
        self.places.count
    }

    /// Decodes the next message of the run. Gives the places and their key set, from
    /// [`KeySet::ordered`], with the last message, else `None`. A refused message
    /// leaves the run as it was.
    ///
    /// # Errors
    ///
    /// In order: the error of its bytes ([`Error::Empty`], [`Error::Length`],
    /// [`Error::Sample`], or [`Error::Name`]), [`Error::Run`] when it holds more
    /// places than remain, and [`Error::Unstamped`] when it holds the index place and
    /// that place's sample type is not `Stamp`. The last message then gives
    /// [`Error::Repeated`] when the places name a channel more than once.
    pub(super) fn decode(
        &mut self,
        message: &[u8],
    ) -> Result<Option<(Vec<Place>, KeySet)>, Error> {
        let Self {
            places,
            remain,
            arrived: all,
        } = self;
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
        // Once for the whole run, so `all` never doubles past the cap, and the last
        // message moves into it before the key set is built.
        all.reserve_exact(to_usize(*remain));
        all.extend(run);
        if rest > 0 {
            *remain = rest;
            return Ok(None);
        }
        let channels = all.iter().map(|place| (place.key, place.data_type));
        match KeySet::ordered(channels, to_usize(places.index)) {
            Ok(set) => Ok(Some((std::mem::take(all), set))),
            Err(repeated) => {
                all.truncate(to_usize(first));
                Err(Error::Repeated { key: repeated.key })
            }
        }
    }
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("invariant: a usize holds a u32")
}
