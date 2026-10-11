//! The messages of a read stream.

use types::channel;
use types::name::{Name, Selector, Written};
use types::sample::Type;

use super::{Body, READ, fields};
use crate::common::Writer;
use crate::hub::{COMPLETE, Error, LATEST};

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
        assert!(
            self.length <= OPEN_BYTES_MAX,
            "an open of {} bytes is over the cap of {OPEN_BYTES_MAX}",
            self.length
        );
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
        let length = u64::from_le_bytes(fields.take()?);
        let signature = fields.take()?;
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
    #[must_use]
    pub fn body(&self) -> Body {
        Body::new(self.length)
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
    /// [`Error::Empty`] when `bytes` is empty, [`Error::Kind`] when the first byte
    /// names no mode, [`Error::Length`] when a pattern ends past the open, and
    /// [`Error::Selector`] when a pattern does not read or none includes names.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (&mode, mut rest) = bytes.split_first().ok_or(Error::Empty)?;
        let mode = match mode {
            LATEST => Mode::Latest,
            COMPLETE => Mode::Complete,
            kind => return Err(Error::Kind { kind }),
        };
        let mut patterns = Vec::new();
        while !rest.is_empty() {
            let length = Error::Length { len: bytes.len() };
            let (&len, after) = rest.split_first_chunk::<2>().ok_or(length)?;
            let (pattern, after) = after
                .split_at_checked(usize::from(u16::from_le_bytes(len)))
                .ok_or(length)?;
            patterns
                .push(std::str::from_utf8(pattern).map_err(|_utf8| Error::Selector)?);
            rest = after;
        }
        let select = Selector::new(patterns).map_err(|_read| Error::Selector)?;
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
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let mut fields = fields(message, READ)?;
        let count = u32::from_le_bytes(fields.take()?);
        let index = u32::from_le_bytes(fields.take()?);
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
    pub sample: Type,
    /// The channel's name.
    pub name: Name,
}

impl Place {
    /// The bytes of the encoded place.
    fn encoded_len(&self) -> usize {
        self.sample
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
    use crate::common::Writer;
    use crate::hub::Error;

    /// `places` cut into the messages of the run, in order: each holds as many whole
    /// places as fit in `max` bytes.
    ///
    /// # Panics
    ///
    /// While it iterates, when a place is over `max` bytes.
    pub fn messages(places: &[Place], max: usize) -> impl Iterator<Item = &[Place]> {
        let mut rest = places;
        std::iter::from_fn(move || {
            let mut bytes = 0_usize;
            let fit = rest
                .iter()
                .take_while(|place| {
                    bytes = bytes.saturating_add(place.encoded_len());
                    bytes <= max
                })
                .count();
            if let Some(first) = rest.first() {
                assert!(
                    fit > 0,
                    "a place of {} bytes is over a message of {max}",
                    first.encoded_len()
                );
            }
            let (message, after) = rest.split_at(fit);
            rest = after;
            (!message.is_empty()).then_some(message)
        })
    }

    /// The bytes of `places`, one message of the run.
    #[must_use]
    pub fn encoded_len(places: &[Place]) -> usize {
        places
            .iter()
            .map(Place::encoded_len)
            .fold(0, usize::saturating_add)
    }

    /// Writes `places` into `out`, one message of the run.
    ///
    /// # Panics
    ///
    /// When `places` is empty, or `out` is not [`encoded_len`] bytes.
    pub fn encode(places: &[Place], out: &mut [u8]) {
        assert!(
            !places.is_empty(),
            "a message of a run holds at least one item"
        );
        let mut out = Writer::new(out, encoded_len(places));
        for place in places {
            let sample = place.sample.to_string();
            out.put(&place.key.as_u128().to_le_bytes());
            out.put(&[u8::try_from(sample.len())
                .expect("invariant: the text of a sample type is under 256 bytes")]);
            out.put(sample.as_bytes());
            out.put(&[u8::try_from(place.name.as_str().len())
                .expect("invariant: a name is at most 255 bytes")]);
            out.put(place.name.as_str().as_bytes());
        }
    }

    /// The places in `message`, one message of the run.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`] when `message` is empty, [`Error::Length`] when it ends inside
    /// a place, and [`Error::Place`] when the sample type or the name of a place does
    /// not read.
    pub fn decode(message: &[u8]) -> Result<Vec<Place>, Error> {
        if message.is_empty() {
            return Err(Error::Empty);
        }
        let length = Error::Length { len: message.len() };
        let mut rest = message;
        let mut places = Vec::new();
        while !rest.is_empty() {
            let (&key, after) = rest.split_first_chunk::<16>().ok_or(length)?;
            let (sample, after) = text(after, length)?;
            let (name, after) = text(after, length)?;
            places.push(Place {
                key: channel::Key::from_u128(u128::from_le_bytes(key)),
                sample: sample.parse().map_err(|_read| Error::Place)?,
                name: name.parse().map_err(|_read| Error::Place)?,
            });
            rest = after;
        }
        Ok(places)
    }

    /// The text at the front of `bytes`, its length a `u8`, and the bytes after it.
    /// Bytes that end inside the text give `length`.
    fn text(bytes: &[u8], length: Error) -> Result<(&str, &[u8]), Error> {
        let (&len, rest) = bytes.split_first().ok_or(length)?;
        let (text, rest) = rest.split_at_checked(usize::from(len)).ok_or(length)?;
        let text = std::str::from_utf8(text).map_err(|_utf8| Error::Place)?;
        Ok((text, rest))
    }
}

#[cfg(test)]
mod tests;
