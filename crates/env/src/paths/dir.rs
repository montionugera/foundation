//! A directory of the host, open.

use std::ffi::OsStr;
use std::fmt::Debug;

use super::{Entry, Error, Key};

/// A directory of the host, open. Each call takes one name in this directory, with no
/// symbolic link at that name followed, so it reaches this directory also after a
/// program moves it or puts another file at its path. Only `os` and `sim` implement
/// it.
///
/// ```
/// fn key(dir: &dyn env::paths::dir::Driver) -> env::paths::Key {
///     dir.key()
/// }
/// ```
pub trait Driver: Debug {
    /// The key of this directory, from the open.
    fn key(&self) -> Key;

    /// What is at `name`, with no symbolic link followed: a directory, open, a link
    /// with its target as written, or another file. For `..`, the directory that the
    /// host gives as the one above this one.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `name`, else [`Error::Io`] with the code
    /// of the host.
    fn open(&self, name: &OsStr) -> Result<Entry, Error>;
}
