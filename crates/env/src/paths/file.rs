//! A file of the host, open.

use std::fmt::Debug;

use super::{Error, Key};

/// A file of the host, open. Only `os` and `sim` implement it.
///
/// ```
/// fn key(file: &dyn env::paths::file::Driver) -> env::paths::Key {
///     file.key()
/// }
/// ```
pub trait Driver: Debug {
    /// The key of this file, from the open.
    fn key(&self) -> Key;

    /// The bytes of this file, or `None` when it holds more than `most`.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] 22 when this file is not a regular file, else [`Error::Io`] with
    /// the code of the host.
    fn read(&self, most: usize) -> Result<Option<Vec<u8>>, Error>;

    /// Writes `bytes` in place: a regular file is cut to them, and a reader, a crash,
    /// or a failed write can see or leave a part of them.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] with the code of the host.
    fn write(&self, bytes: &[u8]) -> Result<(), Error>;
}
