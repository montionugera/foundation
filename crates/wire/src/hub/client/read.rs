//! The messages of a read stream.

use types::channel;
use types::name::{Name, Selector, Written};
use types::sample::Type;

use super::Error;
use super::text;
use super::{Body, READ, fields};
use crate::common::{Fields, Writer};
use crate::hub::{COMPLETE, LATEST};

mod reader;

pub use reader::{FromNode, Reader};

/// The most bytes of the open of a read: 64 KiB.
pub const OPEN_BYTES_MAX: u64 = 1 << 16;

/// The first message of a read stream. The open follows as a body of `length` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    /// The bytes of the open, at most [`OPEN_BYTES_MAX`].
    pub length: u64,
    /// The signature of the hello's key over the signed bytes of the open, of the
    /// kind of a read: the connection and the open.
    pub signature: [u8; 64],
}

impl Read {
    /// The bytes of an encoded read.
    pub const LEN: usize = 73;

    /// Writes the read into `out`.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`], or `out` is not [`Read::LEN`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        self.check();
        let mut out = Writer::new(out, Self::LEN);
        out.put(&[READ]);
        out.put(&self.length.to_le_bytes());
        out.put(&self.signature);
    }

    /// Decodes a read.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a message that is not kind 6,
    /// [`Error::Length`], and [`Error::Oversize`] for an open over
    /// [`OPEN_BYTES_MAX`].
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, READ)?;
        let length = u64::from_le_bytes(*fields.take()?);
        let signature = *fields.take()?;
        fields.end()?;
        if length > OPEN_BYTES_MAX {
            return Err(Error::Oversize {
                length,
                cap: OPEN_BYTES_MAX,
            });
        }
        Ok(Self { length, signature })
    }

    /// The open that follows this read.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`].
    #[must_use]
    pub fn body(&self) -> Body {
        self.check();
        Body::new(self.length)
    }

    fn check(&self) {
        assert!(
            self.length <= OPEN_BYTES_MAX,
            "an open of {} bytes is over the cap of {OPEN_BYTES_MAX}",
            self.length
        );
    }
}

/// Which frames a read gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The newest live frame, before its commit.
    Latest,
    /// Each live frame, after its commit.
    Complete,
}

/// The open of a read: the body after a [`Read`], and what its signature covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    /// Which frames the read gets.
    pub mode: Mode,
    /// The channels it reads, by name. They are on one index.
    pub select: Selector,
}

impl Open {
    /// The bytes of the encoded open.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        self.select
            .written()
            .map(|pattern| {
                let (bang, body) = parts(pattern);
                bang.len().saturating_add(body.len()).saturating_add(2)
            })
            .fold(1, usize::saturating_add)
    }

    /// Writes the open into `out`.
    ///
    /// # Panics
    ///
    /// When `out` is not [`Open::encoded_len`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        let mut out = Writer::new(out, self.encoded_len());
        out.put(&[match self.mode {
            Mode::Latest => LATEST,
            Mode::Complete => COMPLETE,
        }]);
        for pattern in self.select.written() {
            let (bang, body) = parts(pattern);
            let len = u16::try_from(bang.len().saturating_add(body.len()))
                .expect("invariant: a pattern is at most 256 bytes");
            out.put(&len.to_le_bytes());
            out.put(bang.as_bytes());
            out.put(body.as_bytes());
        }
    }

    /// Decodes the open in `bytes`, all the bytes of its body.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `bytes` is empty, [`Error::Mode`] when the first byte
    /// names no mode, [`Error::Length`] when a pattern ends past the open, and
    /// [`Error::Selector`] when a pattern does not read or none includes names.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut fields = Fields::new(bytes);
        let mode = match fields.kind().ok_or(Error::Empty)? {
            LATEST => Mode::Latest,
            COMPLETE => Mode::Complete,
            byte => return Err(Error::Mode { byte }),
        };
        let mut patterns = Vec::new();
        while !fields.is_empty() {
            let len = u16::from_le_bytes(*fields.take()?);
            let pattern = std::str::from_utf8(fields.take_slice(usize::from(len))?)
                .map_err(|error| Error::Selector(text::Error::Utf8(error)))?;
            patterns.push(pattern);
        }
        let select = Selector::new(patterns)
            .map_err(|error| Error::Selector(text::Error::Parse(Box::new(error))))?;
        Ok(Self { mode, select })
    }
}

/// The text of `pattern` as `Selector::new` reads it, in two parts: `!` for an
/// exclusion, and the body.
fn parts(pattern: Written<'_>) -> (&'static str, &str) {
    match pattern {
        Written::Include(body) => ("", body),
        Written::Exclude(body) => ("!", body),
    }
}

/// The first reply on a read stream. The run of its [`place`]s follows, and then the
/// replies of a reader session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Places {
    /// The places of the read, its index too. At least 1.
    pub count: u32,
    /// The place of the index, below `count`.
    pub index: u32,
}

impl Places {
    /// The bytes of an encoded places.
    pub const LEN: usize = 9;

    /// Writes the places into `out`.
    ///
    /// # Panics
    ///
    /// When `index` is not below `count`, or `out` is not [`Places::LEN`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        assert!(
            self.index < self.count,
            "the index is at place {} of {}",
            self.index,
            self.count
        );
        let mut out = Writer::new(out, Self::LEN);
        out.put(&[READ]);
        out.put(&self.count.to_le_bytes());
        out.put(&self.index.to_le_bytes());
    }

    /// Decodes a places.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a message that is not kind 6,
    /// [`Error::Length`], and [`Error::Index`] when `index` is not below `count`.
    pub(crate) fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, READ)?;
        let count = u32::from_le_bytes(*fields.take()?);
        let index = u32::from_le_bytes(*fields.take()?);
        fields.end()?;
        if index >= count {
            return Err(Error::Index { index, count });
        }
        Ok(Self { count, index })
    }
}

/// One place of a read: a channel, in the place order of the read's frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    /// The channel's key.
    pub key: channel::Key,
    /// The channel's sample type.
    pub data_type: Type,
    /// The channel's name.
    pub name: Name,
}

impl Place {
    /// The bytes of the encoded place.
    fn encoded_len(&self) -> usize {
        self.data_type
            .to_string()
            .len()
            .saturating_add(self.name.as_str().len())
            .saturating_add(18)
    }
}

/// The run of places after a [`Places`], in place order. A message of the run holds
/// whole places.
pub mod place {
    use types::channel;

    use super::Place;
    use crate::common::{Fields, Writer};
    use crate::hub::client::Error;
    use crate::hub::client::text;

    /// One message of the run: whole places that fit in the `max` of [`messages`].
    #[derive(Clone, Copy, Debug)]
    pub struct Message<'p> {
        places: &'p [Place],
        len: usize,
    }

    impl Message<'_> {
        /// The bytes of the encoded message.
        #[must_use]
        pub fn encoded_len(&self) -> usize {
            self.len
        }

        /// Writes the message into `out`.
        ///
        /// # Panics
        ///
        /// When `out` is not [`Message::encoded_len`] bytes.
        pub fn encode(&self, out: &mut [u8]) {
            let mut out = Writer::new(out, self.len);
            for place in self.places {
                let data_type = place.data_type.to_string();
                out.put(&place.key.as_u128().to_le_bytes());
                out.put(&[u8::try_from(data_type.len()).expect(
                    "invariant: the text of a sample type is under 256 bytes",
                )]);
                out.put(data_type.as_bytes());
                out.put(&[u8::try_from(place.name.as_str().len())
                    .expect("invariant: a name is at most 255 bytes")]);
                out.put(place.name.as_str().as_bytes());
            }
        }
    }

    /// `places` cut into the messages of the run, in order: each holds as many whole
    /// places as fit in `max` bytes. A place is at most 300 bytes.
    ///
    /// # Panics
    ///
    /// While it iterates, when a place is over `max` bytes. A `max` of at least 300
    /// never panics.
    pub fn messages(places: &[Place], max: usize) -> impl Iterator<Item = Message<'_>> {
        let mut rest = places;
        std::iter::from_fn(move || {
            let mut len = 0_usize;
            let fit = rest
                .iter()
                .take_while(|place| {
                    let bytes = len.saturating_add(place.encoded_len());
                    let fits = bytes <= max;
                    if fits {
                        len = bytes;
                    }
                    fits
                })
                .count();
            if let Some(first) = rest.first() {
                assert!(
                    fit > 0,
                    "a place of {} bytes is over a message of {max}",
                    first.encoded_len()
                );
            }
            let (places, after) = rest.split_at(fit);
            rest = after;
            (!places.is_empty()).then_some(Message { places, len })
        })
    }

    /// The places in `message`, one message of the run.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `message` is empty, [`Error::Length`] when it ends inside
    /// a place, [`Error::Sample`] when the sample type of a place does not read, and
    /// [`Error::Name`] when its name does not. The first defect in the order of the
    /// bytes gives the error.
    pub(crate) fn decode(message: &[u8]) -> Result<Vec<Place>, Error> {
        if message.is_empty() {
            return Err(Error::Empty);
        }
        let mut fields = Fields::new(message);
        let mut places = Vec::new();
        while !fields.is_empty() {
            let key = u128::from_le_bytes(*fields.take()?);
            let [len] = *fields.take()?;
            let data_type = text::read(fields.take_slice(usize::from(len))?)
                .map_err(Error::Sample)?;
            let [len] = *fields.take()?;
            let name = text::read(fields.take_slice(usize::from(len))?)
                .map_err(Error::Name)?;
            places.push(Place {
                key: channel::Key::from_u128(key),
                data_type,
                name,
            });
        }
        Ok(places)
    }
}

#[cfg(test)]
mod tests;
