//! The text fields of a message.

use std::fmt;
use std::str::{FromStr, Utf8Error};

/// Why a text field of a message does not read: `E` is the error of its parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// The bytes are not UTF-8.
    Utf8(Utf8Error),
    /// The text does not parse.
    Parse(E),
}

/// Reads `bytes` as UTF-8, then parses the text as a `T`.
pub(crate) fn read<T: FromStr, E: From<T::Err>>(bytes: &[u8]) -> Result<T, Error<E>> {
    let text = std::str::from_utf8(bytes).map_err(Error::Utf8)?;
    text.parse()
        .map_err(|error: T::Err| Error::Parse(error.into()))
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Utf8(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
        }
    }
}

impl<E: std::error::Error> std::error::Error for Error<E> {}
