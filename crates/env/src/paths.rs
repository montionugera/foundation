//! The files of the host by the paths that a user gave.

use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The files of the host by the paths that a user gave, as on a command line. A
/// relative path is relative to the working directory of the process. A path that
/// ends in `/`, `.`, or `..` names a directory, and the empty path names nothing.
/// Each call blocks its thread until the OS answers, so a shard never calls one: the
/// files of a node are [`crate::files`].
///
/// ```
/// use std::path::Path;
///
/// use env::paths::{Error, Found, Paths};
///
/// fn plan(paths: &Paths) -> Result<Found, Error> {
///     paths.read(Path::new("plant.hcl"), 1 << 24)
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Paths(Rc<dyn Driver>);

impl Paths {
    /// A handle over `driver`. Only `os` and `sim` call it.
    ///
    /// ```
    /// fn wrap(driver: impl env::paths::Driver + 'static) -> env::paths::Paths {
    ///     env::paths::Paths::new(driver)
    /// }
    /// ```
    pub fn new(driver: impl Driver + 'static) -> Self {
        Self(Rc::new(driver))
    }

    /// What is at `path`: the bytes of a file of at most `most` bytes, or the path of
    /// each entry of a directory whose entry paths hold at most `most` bytes in all,
    /// in name order.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, else [`Error::Io`].
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    ///
    /// use env::paths::{Found, Paths};
    ///
    /// fn entries(paths: &Paths) -> Vec<PathBuf> {
    ///     match paths.read(Path::new("plans"), 1 << 16) {
    ///         Ok(Found::Directory(entries)) => entries,
    ///         _ => Vec::new(),
    ///     }
    /// }
    /// ```
    pub fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        self.0.read(path, most)
    }

    /// Writes `bytes` as the file at `path`, whole or not at all, in place of a file
    /// that is there. A link, a device, or a pipe at `path` takes the bytes in place,
    /// so a failure or a second write at the same time can leave a part of them.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the directory of `path`, or the directory that it
    /// names, is not there, else [`Error::Io`].
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use env::paths::{Error, Paths};
    ///
    /// fn out(paths: &Paths, plan: &[u8]) -> Result<(), Error> {
    ///     paths.write(Path::new("plant.plan"), plan)
    /// }
    /// ```
    pub fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        self.0.write(path, bytes)
    }
}

/// What [`Paths::read`] found at a path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    /// A file of at most `most` bytes: its bytes.
    File(Vec<u8>),
    /// A file of more than `most` bytes, or a directory whose entry paths hold more.
    Large,
    /// A directory: the path of each entry, in name order.
    Directory(Vec<PathBuf>),
}

/// Why a call of [`Paths`] failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Nothing is at the path, or the directory of a file to write is not there.
    NotFound,
    /// The OS refused the call with `code`.
    Io {
        /// The OS error code.
        code: i32,
    },
}

/// What `os` and `sim` implement: the calls of [`Paths`], with the same contracts.
///
/// ```
/// fn wrap(driver: impl env::paths::Driver + 'static) -> env::paths::Paths {
///     env::paths::Paths::new(driver)
/// }
/// ```
pub trait Driver: Debug {
    /// [`Paths::read`].
    ///
    /// # Errors
    ///
    /// As [`Paths::read`].
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error>;

    /// [`Paths::write`].
    ///
    /// # Errors
    ///
    /// As [`Paths::write`].
    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// One file, at any path.
    #[derive(Debug, Default)]
    struct One(RefCell<Vec<u8>>);

    impl Driver for One {
        fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
            match path.to_str() {
                Some("") => Err(Error::NotFound),
                Some("dir") => Ok(Found::Directory(vec![path.join("a")])),
                _ if self.0.borrow().len() > most => Ok(Found::Large),
                _ => Ok(Found::File(self.0.borrow().clone())),
            }
        }

        fn write(&self, _: &Path, bytes: &[u8]) -> Result<(), Error> {
            if bytes.is_empty() {
                return Err(Error::Io { code: 28 });
            }
            *self.0.borrow_mut() = bytes.to_vec();
            Ok(())
        }
    }

    #[test]
    fn calls_reach_the_driver_with_their_arguments() {
        let paths = Paths::new(One::default());
        let clone = paths.clone();
        assert_eq!(paths.write(Path::new("a"), b"xyz"), Ok(()));
        assert_eq!(
            clone.read(Path::new("a"), 3),
            Ok(Found::File(b"xyz".to_vec()))
        );
        assert_eq!(clone.read(Path::new("a"), 2), Ok(Found::Large));
        let entries = Found::Directory(vec![PathBuf::from("dir/a")]);
        assert_eq!(paths.read(Path::new("dir"), 0), Ok(entries));
        assert_eq!(paths.read(Path::new(""), 0), Err(Error::NotFound));
        assert_eq!(
            paths.write(Path::new("a"), b""),
            Err(Error::Io { code: 28 })
        );
    }
}
