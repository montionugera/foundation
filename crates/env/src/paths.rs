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
    /// when the folder is the file at the path or a directory that holds it, by the
    /// file and not by its name: a name in other case on a file system that ignores
    /// case, and a bind mount, reach the same folder. Each call takes the file at each
    /// resolved folder again, so a folder that a program removes holds nothing, and a
    /// hidden file that a write replaces stays hidden. A path also fails with
    /// [`Error::Hidden`] when it resolves under the resolved path of a hidden folder,
    /// also when nothing is there, or under the file that was there at this call. The
    /// limits of `self` hold too, so a handle never reaches more than the one it came
    /// from. The check and the call are two steps: a symbolic link that another program
    /// makes between them can lead the call out.
    ///
    /// # Errors
    ///
    /// The error of [`Paths::resolve`] for a path of `folders` or `hidden`, or of
    /// [`Driver::key`] for a path of `folders`, and for a path of `hidden` other than
    /// [`Error::NotFound`]: [`Error::NotFound`] when a folder of `folders` is not
    /// there.
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
        let mut limit = Limit {
            folders: Vec::new(),
            hidden: Vec::new(),
        };
        for dir in folders {
            let dir = self.driver.resolve(dir)?;
            self.driver.key(&dir)?;
            limit.folders.push(dir);
        }
        for dir in hidden {
            let dir = self.driver.resolve(dir)?;
            let key = self.key(&dir)?;
            limit.hidden.push((dir, key));
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
    /// [`Error::Hidden`] past the limits of [`Paths::within`], [`Error::Io`] 22 when
    /// `path` is a file that is neither a regular file nor a directory, such as a FIFO
    /// or a device, else [`Error::Io`]. When another writer changes a link in `path`
    /// during the call, it gives the error of the open of the file that the link then
    /// names, and opens that file, also a device. On Linux, the walk of a link that a
    /// rename replaces during the call can read only a first part of its old target,
    /// also none of it, so the call can open the folder of the link, or a folder on the
    /// way to that target, such as `/`, and the rest of `path` resolves under that
    /// folder. When the link is the last part of `path`, the read gives the entries of
    /// that folder.
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
    /// directory that it makes gives the group and other users no access. On a handle
    /// of [`Paths::within`], a path that does not resolve passes the limits by its own
    /// place, and gives the error of its resolve when it still does not resolve after
    /// the call.
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
            // through a link at `path`, so its directory is what the limits check: no
            // folder is at a name that does not resolve.
            (Err(Error::NotFound | Error::Io { .. }), Some(dir), Some(_)) => {
                let dir = if dir.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    dir
                };
                self.allowed(dir, &self.driver.resolve(dir)?)?;
                self.driver.make_dir(path)?;
                // A link that does not resolve can still lead to a directory, which
                // the place does not hold. A directory made at the place resolves.
                self.check(path)
            }
            (checked, ..) => {
                checked?;
                self.driver.make_dir(path)
            }
        }
    }

    /// `Ok` when `path` passes each limit. A path that does not resolve fails with the
    /// error of its resolve, as the call would.
    fn check(&self, path: &Path) -> Result<(), Error> {
        if self.limits.is_empty() {
            return Ok(());
        }
        self.allowed(path, &self.driver.resolve(path)?)
    }

    /// `Ok` when `path`, which resolves to `resolved`, passes each limit.
    fn allowed(&self, path: &Path, resolved: &Path) -> Result<(), Error> {
        // The key of the file itself comes from `path`, as the call reaches it: the
        // resolved path of a new name can be longer than the host takes.
        let mut keys: Vec<_> = self.key(path)?.into_iter().collect();
        for dir in resolved.ancestors().skip(1) {
            keys.push(self.driver.key(dir)?);
        }
        let holds =
            |dir: &Path| Ok(self.key(dir)?.is_some_and(|key| keys.contains(&key)));
        for limit in self.limits.iter() {
            for (dir, was) in &limit.hidden {
                let named = resolved.starts_with(dir);
                if named || was.is_some_and(|key| keys.contains(&key)) || holds(dir)? {
                    return Err(Error::Hidden);
                }
            }
        }
        for limit in self.limits.iter() {
            let mut inside = false;
            for dir in &limit.folders {
                inside = inside || holds(dir)?;
            }
            if !inside {
                return Err(Error::Outside);
            }
        }
        Ok(())
    }

    /// The key of the file at `dir`, or `None` when nothing is there.
    fn key(&self, dir: &Path) -> Result<Option<Key>, Error> {
        match self.driver.key(dir) {
            Ok(key) => Ok(Some(key)),
            Err(Error::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// The folders of one call of [`Paths::within`].
#[derive(Clone, Debug)]
struct Limit {
    /// The resolved path of each folder.
    folders: Vec<PathBuf>,
    /// The resolved path of each hidden folder, with the key of what was there.
    hidden: Vec<(PathBuf, Option<Key>)>,
}

/// The key of a file of the host: two paths that name the same file have the same
/// key, also through a bind mount or a name in other case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    device: u64,
    inode: u64,
}

impl Key {
    /// The key of file `inode` on the file system `device`. Only `os` and `sim` call
    /// it.
    ///
    /// ```
    /// assert_ne!(env::paths::Key::new(1, 2), env::paths::Key::new(2, 1));
    /// ```
    #[must_use]
    pub const fn new(device: u64, inode: u64) -> Self {
        Self { device, inode }
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

    /// The key of the file at `path`, after each symbolic link.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, else [`Error::Io`].
    fn key(&self, path: &Path) -> Result<Key, Error>;
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

    /// One file, at any path but one under `/none`. A path is under `/`, and `link`
    /// names `/out`. A directory that it makes is the file `dir`. A name in `/none`
    /// resolves only after a directory is made, and `/none/deep`, a link to a
    /// directory that does not resolve, never. The key of a path ignores case, and a
    /// last name that starts with `new` resolves but has no key yet.
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
                _ if path.as_os_str().is_empty() => Err(Error::NotFound),
                _ if path.parent() == Some(Path::new("/none"))
                    && (*self.0.borrow() != b"dir" || path.ends_with("deep")) =>
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

        fn key(&self, path: &Path) -> Result<Key, Error> {
            let name = path.file_name().unwrap_or_default().as_encoded_bytes();
            if name.starts_with(b"new") || path.ends_with("/none/deep") {
                return Err(Error::NotFound);
            }
            let at = self.resolve(path)?;
            // FNV-1a.
            let folded = at.as_os_str().as_encoded_bytes().to_ascii_lowercase();
            let hash = folded
                .iter()
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            Ok(Key::new(0, hash))
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
        assert_eq!(
            paths.make_dir(Path::new("/none/deep")),
            Err(Error::NotFound)
        );
        assert_eq!(paths.make_dir(Path::new("/none/link")), Ok(()));
        let paths = free.within(&["/work".into()], &[]).unwrap();
        assert_eq!(paths.make_dir(Path::new("/none/a")), Err(Error::Outside));
    }

    #[test]
    fn a_folder_that_is_not_there_is_not_found_and_a_hidden_one_hides_its_name() {
        let free = Paths::new(One::default());
        let none = ["/none/a".into()];
        assert_eq!(
            free.within(&["/work".into()], &none).err(),
            Some(Error::NotFound)
        );
        assert_eq!(free.within(&none, &[]).err(), Some(Error::NotFound));
        let new = ["/work/new".into()];
        assert_eq!(free.within(&new, &[]).err(), Some(Error::NotFound));
        let work = ["/work".into()];
        let paths = free.within(&work, &new).unwrap();
        assert_eq!(reached(&paths, "/work/new"), Err(Error::Hidden));
        assert_eq!(reached(&paths, "/work/new2"), Ok(()));
        let limited = free.within(&work, &[]).unwrap();
        let refused = ["/other/new".into()];
        assert_eq!(limited.within(&work, &refused).err(), None);
        let paths = free
            .within(&["/none".into()], &["/work/data".into()])
            .unwrap();
        assert_eq!(paths.read(Path::new("/none/a"), 1), Err(Error::NotFound));
    }

    #[test]
    fn a_hidden_folder_that_the_limits_refuse_and_that_is_there_hides() {
        let free = Paths::new(One::default());
        let outer = free.within(&["/a/b".into()], &[]).unwrap();
        let inner = outer.within(&["/a/b/c".into()], &["/a".into()]).unwrap();
        assert_eq!(reached(&inner, "/a/b/c/d"), Err(Error::Hidden));
        assert_eq!(reached(&outer, "/a/b/c/d"), Ok(()));
    }

    #[test]
    fn a_folder_is_its_file_and_not_its_name() {
        let free = Paths::new(One::default());
        let paths = free
            .within(&["/work".into()], &["/work/data".into()])
            .unwrap();
        for path in ["/WORK", "/Work/a", "/WORK/b/c", "/work/new", "/WORK/DATA2"] {
            assert_eq!(reached(&paths, path), Ok(()), "{path}");
        }
        for path in ["/WORK/DATA", "/work/Data/key", "/Work/data/new"] {
            assert_eq!(reached(&paths, path), Err(Error::Hidden), "{path}");
        }
        assert_eq!(reached(&paths, "/WORKSHOP"), Err(Error::Outside));
    }

    #[test]
    fn a_directory_on_the_way_with_no_key_gives_its_error() {
        let paths = Paths::new(One::default())
            .within(&["/".into()], &[])
            .unwrap();
        assert_eq!(reached(&paths, "/none/deep/a"), Err(Error::NotFound));
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
