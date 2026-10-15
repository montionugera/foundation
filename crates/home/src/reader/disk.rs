//! A read from disk that a complete reader needs before its next frame.

use std::fmt;

use buffer::Mark;
use types::frame::Path;

use super::complete;

/// The read from disk that [`Next::Disk`](super::Next::Disk) asks for. Run
/// [`Read::read`] with no borrow of the shard.
#[derive(Debug)]
pub struct Read {
    pub(super) key: complete::Key,
    pub(super) from: Mark,
    pub(super) budget: usize,
    pub(super) reader: buffer::Reader,
}

impl Read {
    /// Reads the reader's next durable entries of its index, up to a fixed budget of
    /// pool bytes.
    ///
    /// # Errors
    ///
    /// [`Error`] when the buffer's read fails.
    pub async fn read(self) -> Result<Entries, Error> {
        let Self {
            key,
            from,
            budget,
            reader,
        } = self;
        match reader.read(key.slot, Path::Live, from, budget).await {
            Ok(read) => Ok(Entries { key, read }),
            Err(buffer::Error::Files(error)) => Err(Error::Files(error)),
            Err(buffer::Error::Pool(error)) => Err(Error::Pool(error)),
            Err(error) => {
                unreachable!("a buffer read gives only Files and Pool: {error}")
            }
        }
    }
}

/// Why a [`Read::read`] failed. The reader can give no more frames; the shard goes on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A ring read failed, or a commit failed before the read ended.
    Files(env::files::Error),
    /// The pool had no block for the first entry of the read or its table.
    Pool(block::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Files(error) => write!(f, "a read from disk failed: {error}"),
            Self::Pool(error) => write!(f, "the pool has no block for a read: {error}"),
        }
    }
}

impl std::error::Error for Error {}

/// What a [`Read`] got, for [`Shard::fill`](crate::Shard::fill).
#[derive(Debug)]
pub struct Entries {
    pub(crate) key: complete::Key,
    pub(super) read: buffer::Read,
}
