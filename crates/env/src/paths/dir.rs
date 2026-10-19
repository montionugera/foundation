//! A directory of the host, open.

use std::ffi::{OsStr, OsString};
use std::fmt::Debug;
use std::path::PathBuf;

use super::{Error, Key, file};

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

    /// What is at `name`, a name of an entry of this directory and never `.` or `..`,
    /// with no symbolic link followed.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `name`, else [`Error::Io`] with the code
    /// of the host.
    fn open(&self, name: &OsStr) -> Result<Entry, Error>;

    /// The directory that the host gives as the one above this one, open. Above a
    /// root, the root.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the host finds no directory above this one, else
    /// [`Error::Io`] with the code of the host.
    fn up(&self) -> Result<Box<dyn Driver>, Error>;

    /// The absolute path at which the host gives this directory now. It does not check
    /// that the path names this directory: a host can give the last path of a
    /// directory that a program removed.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the host gives no absolute path, else [`Error::Io`]
    /// with the code of the host.
    fn path(&self) -> Result<PathBuf, Error>;

    /// The name of each entry of this directory, other than `.` and `..`, in no order,
    /// or `None` when they hold more than `most` bytes in all.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] with the code of the host, also when this user cannot read this
    /// directory.
    fn names(&self, most: usize) -> Result<Option<Vec<OsString>>, Error>;

    /// The file at `name`, open for `access`, with no symbolic link followed and
    /// nothing cut. An open to read does not wait for a writer of a FIFO; an open to
    /// write waits as a write does.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `name`, else [`Error::Io`] with the code
    /// of the host, also for a link at `name`.
    fn file(
        &self,
        name: &OsStr,
        access: file::Access,
    ) -> Result<Box<dyn file::Driver>, Error>;

    /// Writes `bytes` as a new file, and renames it over what is at `name`, so the
    /// write is whole or not at all. Its permission bits give only this user access.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when this directory is gone, else [`Error::Io`] with the
    /// code of the host, also for a directory at `name`.
    fn replace(&self, name: &OsStr, bytes: &[u8]) -> Result<(), Error>;

    /// Makes the directory `name`, which gives the group and other users no access.
    /// Gives `Ok` when a directory is at `name`, with no link followed.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] with the code of the host, also when another file or a link is at
    /// `name`.
    fn make_dir(&self, name: &OsStr) -> Result<(), Error>;
}

/// What [`Driver::open`] found at a name, with no symbolic link followed.
#[derive(Debug)]
pub enum Entry {
    /// A symbolic link, with its key and the path that it names, as written.
    Link(Key, PathBuf),
    /// A directory, open.
    Directory(Box<dyn Driver>),
    /// A regular file, with its key. A directory that a program puts at the name
    /// during the call can also come back as one.
    File(Key),
    /// A device, a FIFO, or a socket, with its key.
    Special(Key),
}

impl Entry {
    pub(super) fn key(&self) -> Key {
        match self {
            Self::Directory(dir) => dir.key(),
            Self::Link(key, _) | Self::File(key) | Self::Special(key) => *key,
        }
    }
}
