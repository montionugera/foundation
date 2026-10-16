//! The files of the host by the paths that a user gave.

use std::fmt::Debug;
use std::path::{Path, PathBuf, is_separator};
use std::rc::Rc;

/// The files of the host by the paths that a user gave, as on a command line. A
/// relative path is relative to the working directory of the process. A path that
/// ends in `/`, `.`, or `..` names a directory, and the empty path names nothing.
/// Each call blocks its thread until the OS answers, and so delays each task of that
/// thread: a shard of a node never calls one, and the files of a node are
/// [`crate::files`]. A handle from [`Paths::within`] reaches only the paths under its
/// folders.
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
pub struct Paths {
    driver: Rc<dyn Driver>,
    /// The limits of each call of `within` that made this handle: a path passes each.
    limits: Rc<[Limit]>,
}

impl Paths {
    /// A handle over `driver`. Only `os` and `sim` call it.
    ///
    /// ```
    /// fn wrap(driver: impl env::paths::Driver + 'static) -> env::paths::Paths {
    ///     env::paths::Paths::new(driver)
    /// }
    /// ```
    pub fn new(driver: impl Driver + 'static) -> Self {
        Self {
            driver: Rc::new(driver),
            limits: Rc::new([]),
        }
    }

    /// The same files, where each call on a path that does not resolve under one of
    /// `folders` fails with [`Error::Outside`], and one on a path that resolves under
    /// one of `hidden` fails with [`Error::Hidden`]. Such a call reaches no file. Each
    /// folder is resolved as [`Paths::resolve`] gives it, and a path is under a folder
    /// by whole names. The limits of `self` hold too, so a handle never reaches more
    /// than the one it came from. The check and the call are two steps: a symbolic link
    /// that another program makes between them can lead the call out.
    ///
    /// # Errors
    ///
    /// The error of [`Paths::resolve`] for a path of `folders` or `hidden`.
    /// [`Error::NotFound`] when a folder of `hidden` is not there, else [`Error::Io`]
    /// when it is not a directory, or when this process cannot search it. A folder of
    /// `hidden` that the limits of `self` refuse needs no check.
    ///
    /// ```
    /// use std::path::PathBuf;
    ///
    /// use env::paths::{Error, Paths};
    ///
    /// fn tools(paths: &Paths, work: PathBuf, data: PathBuf) -> Result<Paths, Error> {
    ///     paths.within(&[work], &[data])
    /// }
    /// ```
    pub fn within(
        &self,
        folders: &[PathBuf],
        hidden: &[PathBuf],
    ) -> Result<Self, Error> {
        let resolve = |dirs: &[PathBuf]| -> Result<Vec<PathBuf>, Error> {
            dirs.iter().map(|dir| self.driver.resolve(dir)).collect()
        };
        let limit = Limit {
            folders: resolve(folders)?,
            hidden: resolve(hidden)?,
        };
        // A resolve keeps a last name that is not there, as given, and a file system
        // that ignores case then misses the hidden folder in another case. Only a
        // directory at `dir` resolves `dir/..`, and the resolve opens no file.
        for dir in &limit.hidden {
            if self.allowed(dir).is_ok() {
                self.driver.resolve(&dir.join(".."))?;
            }
        }
        Ok(Self {
            driver: Rc::clone(&self.driver),
            limits: self.limits.iter().cloned().chain([limit]).collect(),
        })
    }

    /// The absolute path that `path` names, with each `.`, `..`, and symbolic link
    /// resolved. When nothing is at the last name of `path`, it keeps that name on
    /// its resolved directory. It reads no file and checks no limit of
    /// [`Paths::within`].
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when `path` is empty, when a directory on the way is not
    /// there, or when a symbolic link at its last name names nothing, else
    /// [`Error::Io`].
    ///
    /// ```
    /// use std::path::{Path, PathBuf};
    ///
    /// use env::paths::{Error, Paths};
    ///
    /// fn working(paths: &Paths) -> Result<PathBuf, Error> {
    ///     paths.resolve(Path::new("."))
    /// }
    /// ```
    pub fn resolve(&self, path: &Path) -> Result<PathBuf, Error> {
        self.driver.resolve(path)
    }

    /// The home directory of the user, resolved as [`Paths::resolve`] gives it, or
    /// `None` when the host gives none or it does not resolve.
    ///
    /// ```
    /// fn home(paths: &env::paths::Paths) -> Option<std::path::PathBuf> {
    ///     paths.home()
    /// }
    /// ```
    #[must_use]
    pub fn home(&self) -> Option<PathBuf> {
        self.driver.home()
    }

    /// What is at `path`: the bytes of a file of at most `most` bytes, or the path of
    /// each entry of a directory whose entry paths hold at most `most` bytes in all, in
    /// name order. It reads through one open of `path`: a later rename or replace at
    /// `path` does not change what it reads.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, [`Error::Outside`] or
    /// [`Error::Hidden`] past the limits of [`Paths::within`], else [`Error::Io`].
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
        self.check(path)?;
        self.driver.read(path, most)
    }

    /// Writes `bytes` as the file at `path`, and replaces a file that is there. The
    /// write is whole or not at all when a file or nothing is at `path`. A symbolic
    /// link, a device, or a pipe at `path` takes the bytes in place: a reader, a crash,
    /// or a failed write can see or leave a part of them, and two writes at one time
    /// can leave a mix of the two.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the directory of `path`, the directory that it names,
    /// or the file that a symbolic link at `path` names is not there,
    /// [`Error::Outside`] or [`Error::Hidden`] past the limits of [`Paths::within`],
    /// else [`Error::Io`].
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
        self.check(path)?;
        self.driver.write(path, bytes)
    }

    /// Makes the directory at `path`. Gives `Ok` when a directory is there already. It
    /// makes no directory above it, and none through a symbolic link at `path`. A
    /// directory that it makes gives the group and other users no access.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the directory above `path` is not there,
    /// [`Error::Outside`] or [`Error::Hidden`] past the limits of [`Paths::within`],
    /// else [`Error::Io`], also when a file or a link to nothing is at `path`.
    ///
    /// ```
    /// use std::path::Path;
    ///
    /// use env::paths::{Error, Paths};
    ///
    /// fn data(paths: &Paths) -> Result<(), Error> {
    ///     paths.make_dir(Path::new("data"))
    /// }
    /// ```
    pub fn make_dir(&self, path: &Path) -> Result<(), Error> {
        match (self.check(path), path.parent(), path.file_name()) {
            // A link to nothing or a loop does not resolve. The call makes nothing
            // through a link at `path`, so the place of the link is what the limits
            // check.
            (Err(Error::NotFound | Error::Io { .. }), Some(dir), Some(name)) => {
                let dir = if dir.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    dir
                };
                self.allowed(&self.driver.resolve(dir)?.join(name))?;
            }
            (checked, ..) => checked?,
        }
        self.driver.make_dir(path)
    }

    /// `Ok` when `path` passes each limit. A path that does not resolve fails with the
    /// error of its resolve, as the call would.
    fn check(&self, path: &Path) -> Result<(), Error> {
        if self.limits.is_empty() {
            return Ok(());
        }
        self.allowed(&self.driver.resolve(path)?)
    }

    /// `Ok` when the resolved path `resolved` passes each limit.
    fn allowed(&self, resolved: &Path) -> Result<(), Error> {
        let under = |dirs: &[PathBuf]| dirs.iter().any(|dir| resolved.starts_with(dir));
        if self.limits.iter().any(|limit| under(&limit.hidden)) {
            Err(Error::Hidden)
        } else if self.limits.iter().all(|limit| under(&limit.folders)) {
            Ok(())
        } else {
            Err(Error::Outside)
        }
    }
}

/// The folders of one call of [`Paths::within`].
#[derive(Clone, Debug)]
struct Limit {
    folders: Vec<PathBuf>,
    hidden: Vec<PathBuf>,
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
    /// Nothing is at the path, or the directory or the link target that a write needs
    /// is not there.
    NotFound,
    /// The OS refused the call with `code`.
    Io {
        /// The OS error code.
        code: i32,
    },
    /// The path resolves outside the folders of [`Paths::within`].
    Outside,
    /// The path resolves under a hidden folder of [`Paths::within`].
    Hidden,
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

    /// [`Paths::make_dir`].
    ///
    /// # Errors
    ///
    /// As [`Paths::make_dir`].
    fn make_dir(&self, path: &Path) -> Result<(), Error>;

    /// [`Paths::resolve`].
    ///
    /// # Errors
    ///
    /// As [`Paths::resolve`].
    fn resolve(&self, path: &Path) -> Result<PathBuf, Error>;

    /// [`Paths::home`].
    fn home(&self) -> Option<PathBuf>;
}

/// Whether `path` is empty or ends in a separator, `.`, or `..`. By its text, such a
/// path names a directory or nothing, never a file. It reads no file. A separator is a
/// `/`, and on Windows also a `\` (`std::path::is_separator`). The methods of `Path` do
/// not give this: they drop an end in a separator or `.`.
#[must_use]
pub fn names_directory(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    // A separator is ASCII, and no byte of a longer character is ASCII in these bytes.
    let name = bytes.rsplit(|&byte| is_separator(char::from(byte))).next();
    matches!(name, Some(b"" | b"." | b".."))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// One file, at any path but one under `/none`, and no resolve of a name in
    /// `/none`. A path is under `/`, and `link` names `/out`. A directory that it
    /// makes is the file `dir`.
    #[derive(Debug, Default)]
    struct One(RefCell<Vec<u8>>);

    impl Driver for One {
        fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
            match path.to_str() {
                Some(path) if path.is_empty() || path.starts_with("/none") => {
                    Err(Error::NotFound)
                }
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

        fn make_dir(&self, path: &Path) -> Result<(), Error> {
            self.write(path, b"dir")
        }

        fn resolve(&self, path: &Path) -> Result<PathBuf, Error> {
            match path.strip_prefix("link") {
                _ if path.as_os_str().is_empty()
                    || path.parent() == Some(Path::new("/none")) =>
                {
                    Err(Error::NotFound)
                }
                Ok(rest) => Ok(Path::new("/out").join(rest)),
                Err(_) => Ok(Path::new("/").join(path)),
            }
        }

        fn home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/user"))
        }
    }

    /// What a write, a read, and a `make_dir` of `path` give, which agree.
    fn reached(paths: &Paths, path: &str) -> Result<(), Error> {
        let path = Path::new(path);
        let written = paths.write(path, b"x");
        let read = paths.read(path, 1).map(|_| ());
        let made = paths.make_dir(path);
        assert_eq!([&written, &read], [&made; 2], "{}", path.display());
        written
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
        assert_eq!(paths.make_dir(Path::new("b")), Ok(()));
        assert_eq!(
            paths.read(Path::new("b"), 3),
            Ok(Found::File(b"dir".to_vec()))
        );
    }

    #[test]
    fn resolve_and_home_reach_the_driver_and_check_no_limit() {
        let paths = Paths::new(One::default())
            .within(&["/work".into()], &[])
            .unwrap();
        let resolved = paths.resolve(Path::new("link/a"));
        assert_eq!(resolved, Ok(PathBuf::from("/out/a")));
        assert_eq!(paths.resolve(Path::new("")), Err(Error::NotFound));
        assert_eq!(paths.home(), Some(PathBuf::from("/home/user")));
    }

    #[test]
    fn within_reaches_only_paths_that_resolve_under_a_folder_and_no_hidden_one() {
        let free = Paths::new(One::default());
        let folders = ["work".into(), "/plans".into()];
        let paths = free.within(&folders, &["work/data".into()]).unwrap();
        for path in ["/work", "work/a", "/work/b/c", "/plans/a", "/work/data2"] {
            assert_eq!(reached(&paths, path), Ok(()), "{path}");
        }
        for path in ["/", "/out", "link", "link/a", "/workshop"] {
            assert_eq!(reached(&paths, path), Err(Error::Outside), "{path}");
            assert_eq!(reached(&free, path), Ok(()), "{path}");
        }
        for path in ["/work/data", "work/data/key"] {
            assert_eq!(reached(&paths, path), Err(Error::Hidden), "{path}");
            assert_eq!(reached(&free, path), Ok(()), "{path}");
        }
        assert_eq!(reached(&paths.clone(), "/out"), Err(Error::Outside));
        assert_eq!(paths.read(Path::new(""), 1), Err(Error::NotFound));
        assert_eq!(paths.write(Path::new(""), b"x"), Err(Error::NotFound));
    }

    #[test]
    fn a_folder_that_does_not_resolve_gives_its_error() {
        let free = Paths::new(One::default());
        let empty = [PathBuf::new()];
        assert_eq!(free.within(&empty, &[]).err(), Some(Error::NotFound));
        let folders = ["/work".into()];
        assert_eq!(free.within(&folders, &empty).err(), Some(Error::NotFound));
    }

    #[test]
    fn make_dir_checks_a_path_that_does_not_resolve_by_its_place() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/none".into()], &[]).unwrap();
        assert_eq!(paths.make_dir(Path::new("/none/link")), Ok(()));
        let paths = free.within(&["/work".into()], &[]).unwrap();
        assert_eq!(paths.make_dir(Path::new("/none/a")), Err(Error::Outside));
    }

    #[test]
    fn a_hidden_folder_that_is_not_there_is_not_found() {
        let free = Paths::new(One::default());
        let none = ["none".into()];
        assert_eq!(
            free.within(&["/work".into()], &none).err(),
            Some(Error::NotFound)
        );
        let paths = free.within(&none, &["/work/data".into()]).unwrap();
        assert_eq!(paths.read(Path::new("/none/a"), 1), Err(Error::NotFound));
    }

    #[test]
    fn a_path_outside_reaches_no_file() {
        let free = Paths::new(One::default());
        free.write(Path::new("/out/a"), b"old").unwrap();
        let paths = free.within(&["/work".into()], &[]).unwrap();
        assert_eq!(
            paths.write(Path::new("/out/a"), b"new"),
            Err(Error::Outside)
        );
        assert_eq!(paths.make_dir(Path::new("/out/a")), Err(Error::Outside));
        assert_eq!(
            free.read(Path::new("/out/a"), 3),
            Ok(Found::File(b"old".to_vec()))
        );
    }

    #[test]
    fn a_handle_within_another_keeps_its_limits() {
        let free = Paths::new(One::default());
        let wide = free.within(&["/".into()], &["/work/data".into()]).unwrap();
        let narrow = wide.within(&["/work".into()], &[]).unwrap();
        assert_eq!(reached(&narrow, "/work/a"), Ok(()));
        assert_eq!(reached(&narrow, "/out"), Err(Error::Outside));
        assert_eq!(reached(&narrow, "/work/data"), Err(Error::Hidden));
        let beside = wide.within(&["/plans".into()], &[]).unwrap();
        assert_eq!(reached(&beside, "/work/data"), Err(Error::Hidden));
        let widened = narrow.within(&["/".into()], &[]).unwrap();
        assert_eq!(reached(&widened, "/out"), Err(Error::Outside));
        assert_eq!(reached(&wide, "/out"), Ok(()));
    }

    #[test]
    fn a_path_that_is_empty_or_ends_in_a_slash_or_dots_names_a_directory() {
        for path in ["", "/", "a/", "a//", ".", "..", "a/.", "a/.."] {
            assert!(names_directory(Path::new(path)), "{path:?}");
        }
        for path in ["a", "/a", "a/b", ".a", "a.", "..a", "a/..."] {
            assert!(!names_directory(Path::new(path)), "{path:?}");
        }
        for path in ["a\\", "a\\.."] {
            assert_eq!(names_directory(Path::new(path)), cfg!(windows), "{path:?}");
        }
    }
}
