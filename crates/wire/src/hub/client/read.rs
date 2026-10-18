//! The messages of a read stream.

use types::name::{Selector, Written};

use super::{Body, Error, READ, stream, text};
use crate::common::{Fields, Writer};
use crate::hub::{COMPLETE, LATEST};

mod reader;

pub use reader::{Reader, Start};

/// The first message of a read stream. The open follows as a body of `length` bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Read {
    /// The bytes of the open, at most [`OPEN_BYTES_MAX`](super::OPEN_BYTES_MAX).
    pub length: u64,
    /// The signature of the hello's key over the signed bytes of the open, of the
    /// kind of a read: the connection and the open.
    pub signature: [u8; 64],
}

impl Read {
    /// The bytes of an encoded read.
    pub const LEN: usize = stream::LEN;

    /// Writes the read into `out`.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`](super::OPEN_BYTES_MAX), or `out` is not
    /// [`Read::LEN`] bytes.
    pub fn encode(&self, out: &mut [u8]) {
        stream::encode(READ, self.length, &self.signature, out);
    }

    /// Decodes a read.
    ///
    /// # Errors
    ///
    /// [`Error::Empty`], [`Error::Kind`] for a message that is not kind 6,
    /// [`Error::Length`], and [`Error::Oversize`] for an open over
    /// [`OPEN_BYTES_MAX`](super::OPEN_BYTES_MAX).
    pub fn decode(message: &[u8]) -> Result<Self, Error> {
        let (length, signature) = stream::decode(READ, message)?;
        Ok(Self { length, signature })
    }

    /// The open that follows this read.
    ///
    /// # Panics
    ///
    /// When `length` is over [`OPEN_BYTES_MAX`](super::OPEN_BYTES_MAX).
    #[must_use]
    pub fn body(&self) -> Body {
        stream::body(self.length)
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

#[cfg(test)]
mod tests;
