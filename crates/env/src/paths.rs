//! The files of the host by the paths that a user gave.

use std::fmt::Debug;
use std::iter;
use std::path::{Path, PathBuf, is_separator};
use std::rc::Rc;

use walk::{Step, Walk};

mod walk;

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

    /// The same files, where a call reaches a file only when each step of the walk of
    /// its path passes the limits of this call and of `self`, in order. The walk takes
    /// the path name by name, as [`Paths::resolve`] does, also through each symbolic
    /// link target. Its steps are the root, each name that it reaches, at its resolved
    /// path, and the step where the path ends. The first step that fails gives its
    /// error, and the call reaches no file:
    ///
    /// - [`Error::Outside`]: no folder of `folders` holds the step. A folder holds a
    ///   step when it is the file at the step or a directory on the way to it, by the
    ///   file and not by its name: a name in other case on a file system that ignores
    ///   case, and a bind mount, reach the same folder. The step where the path ends
    ///   must be held. Another step is checked only when a `..` takes the walk above
    ///   it, nothing is there, or a hidden folder holds it, and then it also passes as
    ///   a directory on the way to a folder that is there.
    /// - [`Error::Replaced`]: in place of `Outside`, when the file at the resolved path
    ///   of a folder is not the one that was there at this call, so that folder holds
    ///   nothing, and the step is under that folder, by the path that it resolved to at
    ///   this call or by the one that it resolves to now, or in the directory that it
    ///   was, or a step short of the end is on the way to the path that it resolved
    ///   to. A program that renames, replaces, or removes a folder does this, unless
    ///   the host gives the new folder the key of the old one, as ext4 can. A write
    ///   replaces a folder that is a file, also through this handle.
    /// - [`Error::Hidden`]: the step passes the folders and a hidden folder holds it.
    ///   Each call walks each path of `hidden` again, so a hidden file that a write
    ///   replaces, and a hidden folder made after this call, are hidden. A directory
    ///   on the way that was at a hidden path at this call hides too, so a hidden
    ///   folder that a program renames away stays hidden, but not its own listing.
    /// - [`Error::Vacant`]: while nothing is at a hidden path, a write or a `make_dir`
    ///   whose path ends at a new name in the deepest directory on the way to that
    ///   path. A read there finds nothing, and fails with [`Error::NotFound`].
    ///
    /// A folder whose file a call cannot take holds nothing at that call. When the
    /// steps pass and the walk fails, the call gives the error of the walk. The check
    /// and the call are two steps: a process that can write a symbolic link in a
    /// folder between them can lead the call to a path that the check refused (#3094).
    ///
    /// # Errors
    ///
    /// The error of [`Paths::resolve`] for a path of `folders` or `hidden`, and
    /// [`Error::NotFound`] when nothing is at a path of `folders`.
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
            let end = self.walk(dir).into_end()?;
            limit
                .folders
                .push((end.path, end.own.ok_or(Error::NotFound)?));
        }
        for given in hidden {
            let end = self.walk(given).into_end()?;
            limit.hidden.push(Hidden {
                given: given.clone(),
                was: end.own,
            });
        }
        Ok(Self {
            driver: Rc::clone(&self.driver),
            limits: self.limits.iter().cloned().chain([limit]).collect(),
        })
    }

    /// The absolute path that `path` names, with each `.`, `..`, and symbolic link
    /// resolved, name by name as the host takes them. When nothing is at the last name
    /// of `path`, also through a symbolic link, it keeps that name on its resolved
    /// directory. It reads no file and checks no limit of [`Paths::within`].
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when `path` is empty or a directory on the way is not there,
    /// the error of [`Driver::looped`] past [`LINKS`] symbolic links, else
    /// [`Error::Io`], also for a `.`, a `..`, or an end in `/` after a file. Each name
    /// goes to the host by its resolved path, so a name whose resolved path is longer
    /// than the host takes fails with [`Error::Io`].
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
        Ok(self.walk(path).into_end()?.path)
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
        self.resolve(&self.driver.home()?).ok()
    }

    /// What is at `path`: the bytes of a file of at most `most` bytes, or the path of
    /// each entry of a directory whose entry paths hold at most `most` bytes in all, in
    /// name order. It reads through one open of `path`: a later rename or replace at
    /// `path` does not change what it reads.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, [`Error::Outside`],
    /// [`Error::Hidden`], or [`Error::Replaced`] past the limits of [`Paths::within`],
    /// [`Error::Io`] 22 when `path` is a file that is neither a regular file nor a
    /// directory, such as a FIFO or a device, else [`Error::Io`]. When another writer
    /// changes a link in `path` during the call, it gives the error of the open of the
    /// file that the link then names, and opens that file, also a device. On Linux, the
    /// walk of a link that a rename replaces during the call can read only a first part
    /// of its old target, also none of it, so the call can open the folder of the link,
    /// or a folder on the way to that target, such as `/`, and the rest of `path`
    /// resolves under that folder. When the link is the last part of `path`, the read
    /// gives the entries of that folder.
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
        self.check(path, Call::Read)??;
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
    /// [`Error::Outside`], [`Error::Hidden`], [`Error::Vacant`], or [`Error::Replaced`]
    /// past the limits of [`Paths::within`], else [`Error::Io`].
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
        self.check(path, Call::Make)??;
        self.driver.write(path, bytes)
    }

    /// Makes the directory at `path`. Gives `Ok` when a directory is there already. It
    /// makes no directory above it, and none through a symbolic link at `path`. A
    /// directory that it makes gives the group and other users no access. On a handle
    /// of [`Paths::within`], `path` first passes the limits of [`Paths::within`]. When
    /// the walk of `path` fails at its last name and the directory is made, the call
    /// checks `path` again and gives the error of that check.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the directory above `path` is not there,
    /// [`Error::Outside`], [`Error::Hidden`], [`Error::Vacant`], or [`Error::Replaced`]
    /// past the limits of [`Paths::within`], else [`Error::Io`], also when a file or a
    /// link to nothing is at `path`.
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
        match self.check(path, Call::Make)? {
            // The host takes the last name with no link followed, so its error can
            // differ from that of the walk.
            Err(_) => {
                self.driver.make_dir(path)?;
                self.check(path, Call::Make)?
            }
            Ok(()) => self.driver.make_dir(path),
        }
    }

    /// `Ok` with the end of the walk of `path` when each step of it passes each limit
    /// for `call`, else the error of the first step that fails.
    fn check(&self, path: &Path, call: Call) -> Result<Result<(), Error>, Error> {
        if self.limits.is_empty() {
            return Ok(Ok(()));
        }
        let walk = self.walk(path);
        let now: Vec<_> = self.limits.iter().map(|limit| self.now(limit)).collect();
        for (index, step) in walk.steps.iter().enumerate() {
            let ask = if index + 1 == walk.steps.len() && walk.end.is_ok() {
                Ask::Hold(call)
            } else if step.back || step.own.is_none() {
                Ask::Way
            } else {
                Ask::Pass
            };
            self.allowed(&walk.steps, index, ask, &now)?;
        }
        walk.end
    }

    /// `Ok` when step `index` of `steps` passes each limit as `ask` gives, where `now`
    /// is what this call found of each limit.
    fn allowed(
        &self,
        steps: &[Step],
        index: usize,
        ask: Ask,
        now: &[Now],
    ) -> Result<(), Error> {
        let step = &steps[index];
        let own = step.own;
        let dirs: Vec<_> = iter::successors(step.up, |&up| steps[up].up)
            .filter_map(|up| steps[up].own)
            .collect();
        let keys: Vec<_> = own.into_iter().chain(dirs.iter().copied()).collect();
        let (mut vacant, mut veiled) = (false, false);
        for (limit, now) in self.limits.iter().zip(now) {
            for (Hidden { was, .. }, seen) in limit.hidden.iter().zip(&now.hidden) {
                let moved = was.is_some_and(|key| dirs.contains(&key));
                let hidden = match seen {
                    Seen::There(key) => keys.contains(key),
                    Seen::Vacant(deepest) => {
                        vacant |= own.is_none() && dirs.first() == deepest.as_ref();
                        false
                    }
                    Seen::Refused => false,
                };
                veiled |= moved || hidden;
            }
        }
        // A step that a hidden folder holds gets `Hidden` only where it passes the
        // folders, and no later step speaks for a path through it.
        let ask = if veiled && ask == Ask::Pass {
            Ask::Way
        } else {
            ask
        };
        let passes = |((_, was), folder): (&(PathBuf, Key), &Folder)| {
            folder.there
                && (keys.contains(was)
                    || ask == Ask::Way
                        && own.is_some_and(|own| folder.above.contains(&own)))
        };
        if ask != Ask::Pass
            && let Some((limit, _)) =
                self.limits.iter().zip(now).find(|(limit, now)| {
                    !limit.folders.iter().zip(&now.folders).any(passes)
                })
        {
            let replaced = limit.folders.iter().any(|(dir, was)| {
                keys.contains(was)
                    || step.path.starts_with(dir)
                    || !matches!(ask, Ask::Hold(_)) && dir.starts_with(&step.path)
                    || self.resolve(dir).is_ok_and(|to| step.path.starts_with(to))
            });
            return Err(if replaced {
                Error::Replaced
            } else {
                Error::Outside
            });
        }
        if veiled {
            return Err(Error::Hidden);
        }
        if vacant && ask == Ask::Hold(Call::Make) {
            return Err(Error::Vacant);
        }
        Ok(())
    }

    /// What this call finds of the folders of `limit`.
    fn now(&self, limit: &Limit) -> Now {
        let key = |path: &Path| match self.driver.entry(path) {
            Ok(Entry::Directory(key) | Entry::File(key)) => Some(key),
            _ => None,
        };
        let folders = limit.folders.iter().map(|(dir, was)| Folder {
            there: key(dir) == Some(*was),
            above: dir.ancestors().skip(1).filter_map(key).collect(),
        });
        let hidden = limit.hidden.iter().map(|Hidden { given, .. }| {
            let walk = self.walk(given);
            let deepest = |step: &Step| {
                let up = &walk.steps[step.up?];
                up.directory.then_some(up.own).flatten()
            };
            match (&walk.end, walk.steps.last()) {
                (Ok(Ok(())), Some(Step { own: Some(key), .. })) => Seen::There(*key),
                (
                    Ok(Ok(())) | Err(Error::NotFound),
                    Some(step @ Step { own: None, .. }),
                ) => Seen::Vacant(deepest(step)),
                _ => Seen::Refused,
            }
        });
        Now {
            folders: folders.collect(),
            hidden: hidden.collect(),
        }
    }

    fn walk(&self, path: &Path) -> Walk {
        Walk::new(&*self.driver, path)
    }
}

/// What a call of [`Paths`] does at its path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Call {
    Read,
    /// A write or a `make_dir`, which can make a new name.
    Make,
}

/// What the folders ask of a step of the walk of [`Paths::within`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ask {
    /// The step where the path ends: a folder holds it.
    Hold(Call),
    /// A `..` takes the walk above the step, nothing is there, or a hidden folder
    /// holds it: a folder holds it, or it is a directory on the way to one that is
    /// there.
    Way,
    /// The step changes where the path ends only as a link or a mount, and the end
    /// meets the folders for it.
    Pass,
}

/// A hidden folder of one call of [`Paths::within`].
#[derive(Clone, Debug)]
struct Hidden {
    /// The path that the caller gave, which each call walks again.
    given: PathBuf,
    /// The key of what was there at `within`.
    was: Option<Key>,
}

/// The folders of one call of [`Paths::within`].
#[derive(Clone, Debug)]
struct Limit {
    /// The resolved path of each folder, with the key of what was there.
    folders: Vec<(PathBuf, Key)>,
    hidden: Vec<Hidden>,
}

/// What one call of [`Paths`] finds of the folders of a [`Limit`], in their order.
struct Now {
    folders: Vec<Folder>,
    hidden: Vec<Seen>,
}

/// What one call finds of a folder of `folders`.
struct Folder {
    /// The file at its resolved path is the one that was there at `within`.
    there: bool,
    /// The key of each directory above its resolved path that the host gives.
    above: Vec<Key>,
}

/// What one call finds at a hidden path.
enum Seen {
    There(Key),
    /// Nothing, with the key of the deepest directory on the way that is there.
    Vacant(Option<Key>),
    /// The host refused the path.
    Refused,
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
    /// A file of more than `most` bytes, or a directory whose entry paths hold more:
    /// which of the two.
    Large(Kind),
    /// A directory: the path of each entry, in name order.
    Directory(Vec<PathBuf>),
}

/// What is at a path: a file or a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
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
    /// A step of the walk of the path is not under the folders of [`Paths::within`].
    Outside,
    /// A step of the walk of the path is under a hidden folder of [`Paths::within`].
    Hidden,
    /// Nothing is at a hidden path of [`Paths::within`], and a write or a
    /// [`Paths::make_dir`] makes a new name in the deepest directory on the way to it
    /// that is there.
    Vacant,
    /// No folder of [`Paths::within`] holds the path, and a program moved or replaced a
    /// folder of that call after it: the folder's path names another file, or nothing,
    /// or the OS refuses its key. The path resolves under that folder, by the path that
    /// the folder resolved to at that call or by the one that it resolves to now, or
    /// into the directory that the folder was at that call, or the path of a step of it
    /// short of the whole path that is checked against the folders is on the way to the
    /// path that the folder resolved to at that call.
    Replaced,
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

    /// The home directory of the user as the host gives it, or `None` when it gives
    /// none.
    fn home(&self) -> Option<PathBuf>;

    /// What is at `path`, with no symbolic link at its last name followed. A path that
    /// ends in `/.` or `/..` names the directory that the host resolves for it.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, else [`Error::Io`], also when a
    /// file is on the way, with the code of the host.
    fn entry(&self, path: &Path) -> Result<Entry, Error>;

    /// The absolute path that the host walks for `path`, as text: a relative path on
    /// the resolved working directory. It reads no file other than the working
    /// directory. On a host other than Windows, it keeps each `..`, and it gives an end
    /// `/` or `/.` as an end `/`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the working directory is gone, else [`Error::Io`].
    fn absolute(&self, path: &Path) -> Result<PathBuf, Error>;

    /// The error that the host gives for a path through more than [`LINKS`] symbolic
    /// links.
    fn looped(&self) -> Error;
}

/// The most symbolic links that the walk of one path goes through, as on Linux. A
/// host can take fewer.
pub const LINKS: usize = 40;

/// What [`Driver::entry`] found at a path, with no symbolic link followed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A symbolic link, with its key and the path that it names, as written.
    Link(Key, PathBuf),
    /// A directory, with its key.
    Directory(Key),
    /// Any other file, also a device or a FIFO, with its key.
    File(Key),
}

impl Entry {
    fn key(&self) -> Key {
        match self {
            Self::Link(key, _) | Self::Directory(key) | Self::File(key) => *key,
        }
    }
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
    use std::cell::{Cell, RefCell};

    use super::*;

    /// One file, at any path but one under `/none`, where each other name is a
    /// directory, other than a name that ends in `.hcl`, which is a file, and a path
    /// with a name after one, which gives `Io` 20. After a write of `loop`, the name
    /// `loop` is a link to itself. `c<n>` for n from 1 is a link to `c<n - 1>`, and
    /// `link` a link to `/out`.
    /// `/none/deep` is a link to `/none/new/a`. A directory that it makes is the file
    /// `dir`. A name in `/none` is there only while the last write made a directory,
    /// and a name that starts with `new` never. The key of a path ignores case. The
    /// name `Gone`, in that case, gives `Io` 13 after a write of `gone`. The key of a
    /// name that starts with `swap` in any case takes the bytes of the file after the
    /// path, so a write gives it a new key: after a write of `x`, `/work/swap` has the
    /// key that `/work/swapx` had before it. The name `io` gives `Io` 5. After a write
    /// of `hop`, `/hop` is a link to `/out`. A path in `/volume` has the key of the path
    /// without `/volume`, as a second mount of `/` does. It counts its calls of
    /// `entry`, and a relative path is under its working directory, `/` at first.
    #[derive(Debug, Default)]
    struct One(RefCell<Vec<u8>>, Rc<Cell<usize>>, Rc<RefCell<PathBuf>>);

    impl Driver for One {
        fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
            match path.to_str() {
                Some(path) if path.is_empty() || path.starts_with("/none") => {
                    Err(Error::NotFound)
                }
                Some("dir") => Ok(Found::Directory(vec![path.join("a")])),
                _ if self.0.borrow().len() > most => Ok(Found::Large(Kind::File)),
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

        fn home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/user"))
        }

        fn entry(&self, path: &Path) -> Result<Entry, Error> {
            self.1.set(self.1.get() + 1);
            let data = self.0.borrow();
            let name = path.file_name().unwrap_or_default().as_encoded_bytes();
            if name == b"io" {
                return Err(Error::Io { code: 5 });
            }
            let bytes = path.as_os_str().as_encoded_bytes();
            if bytes.windows(5).any(|part| part == b".hcl/") {
                return Err(Error::Io { code: 20 });
            }
            if name == b"Gone" && *data == b"gone" {
                return Err(Error::Io { code: 13 });
            }
            let in_none = path.parent() == Some(Path::new("/none")) && *data != b"dir";
            if name.starts_with(b"new") || in_none && path != Path::new("/none/deep") {
                return Err(Error::NotFound);
            }
            let mut at = path.to_path_buf();
            if let Ok(rest) = path.strip_prefix("/volume")
                && rest != Path::new("")
            {
                at = Path::new("/").join(rest);
            }
            let mut folded = at.as_os_str().as_encoded_bytes().to_ascii_lowercase();
            if name.to_ascii_lowercase().starts_with(b"swap") {
                folded.extend_from_slice(&data);
            }
            // FNV-1a.
            let hash = folded
                .iter()
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            let key = Key::new(0, hash);
            Ok(match path.to_str() {
                Some("/none/deep") => Entry::Link(key, "/none/new/a".into()),
                Some("/hop") if *data == b"hop" => Entry::Link(key, "/out".into()),
                _ if name == b"link" => Entry::Link(key, "/out".into()),
                _ if name == b"loop" && *data == b"loop" => {
                    Entry::Link(key, "loop".into())
                }
                _ if name.ends_with(b".hcl") => Entry::File(key),
                _ => match name
                    .strip_prefix(b"c")
                    .and_then(|n| str::from_utf8(n).ok()?.parse::<u8>().ok())
                {
                    Some(n @ 1..) => Entry::Link(key, format!("c{}", n - 1).into()),
                    _ => Entry::Directory(key),
                },
            })
        }

        fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
            Ok(Path::new("/").join(&*self.2.borrow()).join(path))
        }

        fn looped(&self) -> Error {
            Error::Io { code: 40 }
        }
    }

    /// What a write and a `make_dir` of `path` give, which agree. A read gives the
    /// same, but passes the limits where they give `Vacant`.
    fn reached(paths: &Paths, path: &str) -> Result<(), Error> {
        let path = Path::new(path);
        let written = paths.write(path, b"x");
        let read = paths.read(path, 1).map(|_| ());
        let made = paths.make_dir(path);
        let passed = if written == Err(Error::Vacant) {
            Ok(())
        } else {
            written
        };
        assert_eq!([&made, &read], [&written, &passed], "{}", path.display());
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
        assert_eq!(clone.read(Path::new("a"), 2), Ok(Found::Large(Kind::File)));
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
    fn a_folder_whose_key_the_os_refuses_is_replaced() {
        let free = Paths::new(One::default());
        let within = free.within(&["/Gone".into()], &[]).unwrap();
        free.write(Path::new("/x"), b"gone").unwrap();
        let read = within.read(Path::new("/gone/a"), 1).map(|_| ());
        assert_eq!(read, Err(Error::Replaced));
    }

    #[test]
    fn make_dir_checks_a_path_that_does_not_resolve_by_its_place() {
        // A new `One` for each path: a made directory makes each name in `/none`
        // resolve.
        let within = |folder: &str| {
            let free = Paths::new(One::default());
            free.within(&[folder.into()], &[]).unwrap()
        };
        assert_eq!(
            within("/none").make_dir(Path::new("/none/deep")),
            Err(Error::NotFound)
        );
        assert_eq!(within("/none").make_dir(Path::new("/none/link")), Ok(()));
        for path in ["/none/a", "/none/deep"] {
            let made = within("/work").make_dir(Path::new(path));
            assert_eq!(made, Err(Error::Outside), "{path}");
        }
    }

    #[test]
    fn a_missing_folder_is_not_found_and_a_new_name_by_a_hidden_one_is_vacant() {
        let free = Paths::new(One::default());
        let none = ["/none/x/a".into()];
        assert_eq!(
            free.within(&["/work".into()], &none).err(),
            Some(Error::NotFound)
        );
        let none = ["/none/a".into()];
        assert_eq!(free.within(&none, &[]).err(), Some(Error::NotFound));
        let new = ["/work/new".into()];
        assert_eq!(free.within(&new, &[]).err(), Some(Error::NotFound));
        let work = ["/work".into()];
        let paths = free.within(&work, &new).unwrap();
        for path in ["/work/new", "/work/new2"] {
            assert_eq!(reached(&paths, path), Err(Error::Vacant), "{path}");
        }
        assert_eq!(reached(&paths, "/work/a"), Ok(()));
        assert_eq!(reached(&paths, "/work/b/new"), Ok(()));
        let limited = free.within(&work, &[]).unwrap();
        let refused = ["/other/new".into()];
        assert_eq!(limited.within(&work, &refused).err(), None);
        let paths = free
            .within(&["/none".into()], &["/work/data".into()])
            .unwrap();
        assert_eq!(paths.read(Path::new("/none/a"), 1), Err(Error::NotFound));
    }

    #[test]
    fn within_gives_the_error_of_a_hidden_key_other_than_not_found() {
        let free = Paths::new(One::default());
        let within = free.within(&["/work".into()], &["/work/io".into()]);
        assert_eq!(within.err(), Some(Error::Io { code: 5 }));
    }

    #[test]
    fn a_hidden_file_that_a_write_replaces_hides_by_its_new_key() {
        let free = Paths::new(One::default());
        let paths = free
            .within(&["/work".into()], &["/work/swap".into()])
            .unwrap();
        free.write(Path::new("/work/a"), b"y").unwrap();
        assert_eq!(reached(&paths, "/WORK/SWAP"), Err(Error::Hidden));
    }

    #[test]
    fn a_hidden_folder_that_moves_hides_the_files_under_it_but_not_itself() {
        let free = Paths::new(One::default());
        let paths = free
            .within(&["/work".into()], &["/work/swapx".into()])
            .unwrap();
        free.write(Path::new("/work/a"), b"x").unwrap();
        assert_eq!(reached(&paths, "/work/swap/a"), Err(Error::Hidden));
        assert_eq!(
            paths.read(Path::new("/work/swap"), 1),
            Ok(Found::File(b"x".into()))
        );
    }

    #[test]
    fn a_folder_that_is_replaced_holds_nothing() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/work/swapx".into()], &[]).unwrap();
        free.write(Path::new("/work/a"), b"x").unwrap();
        assert_eq!(reached(&paths, "/work/swap/a"), Err(Error::Replaced));
        assert_eq!(reached(&paths, "/work/swapx/a"), Err(Error::Replaced));
    }

    #[test]
    fn a_folder_that_a_link_replaces_holds_nothing() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/hop".into()], &[]).unwrap();
        free.write(Path::new("/x"), b"hop").unwrap();
        let read = paths.read(Path::new("/hop/a"), 3).map(|_| ());
        assert_eq!(read, Err(Error::Replaced));
    }

    #[test]
    fn a_folder_that_is_gone_is_replaced_at_its_path() {
        let free = Paths::new(One::default());
        free.make_dir(Path::new("/none/gone")).unwrap();
        let paths = free.within(&["/none/gone".into()], &[]).unwrap();
        free.write(Path::new("/a"), b"x").unwrap();
        for path in ["/none/gone", "/none/gone/a"] {
            assert_eq!(reached(&paths, path), Err(Error::Replaced), "{path}");
        }
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
    fn a_hidden_folder_above_a_folder_is_outside_as_the_whole_path() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/a/b".into()], &["/a".into()]).unwrap();
        assert_eq!(reached(&paths, "/a"), Err(Error::Outside));
        assert_eq!(reached(&paths, "/a/f"), Err(Error::Hidden));
        assert_eq!(reached(&paths, "/a/b/c/d"), Err(Error::Hidden));
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
    fn a_step_before_a_dot_dot_passes_on_the_way_to_a_folder() {
        let paths = Paths::new(One::default())
            .within(&["/work".into()], &[])
            .unwrap();
        assert_eq!(reached(&paths, "/work/../work/a"), Ok(()));
    }

    #[test]
    fn a_folder_holds_its_files_through_another_mount() {
        let one = One::default();
        let working = Rc::clone(&one.2);
        let paths = Paths::new(one)
            .within(&["/work".into()], &["/work/data".into()])
            .unwrap();
        assert_eq!(reached(&paths, "/volume/work/a"), Ok(()));
        assert_eq!(reached(&paths, "/volume/work/data/key"), Err(Error::Hidden));
        assert_eq!(reached(&paths, "/volume/out"), Err(Error::Outside));
        *working.borrow_mut() = PathBuf::from("/volume/work");
        assert_eq!(reached(&paths, "a"), Ok(()));
        assert_eq!(reached(&paths, "data/key"), Err(Error::Hidden));
        *working.borrow_mut() = PathBuf::from("/volume/out");
        assert_eq!(reached(&paths, "a"), Err(Error::Outside));
    }

    #[test]
    fn a_hidden_folder_meets_only_a_step_inside_the_folders() {
        let one = One::default();
        let working = Rc::clone(&one.2);
        let hidden = ["/work/data".into(), "/out/h".into()];
        let paths = Paths::new(one).within(&["/work".into()], &hidden).unwrap();
        assert_eq!(reached(&paths, "/out/h/a"), Err(Error::Outside));
        *working.borrow_mut() = PathBuf::from("/work/data");
        assert_eq!(reached(&paths, "link/a"), Err(Error::Hidden));
    }

    #[test]
    fn a_dot_dot_that_stays_in_another_mount_of_a_folder_passes() {
        let one = One::default();
        let working = Rc::clone(&one.2);
        let paths = Paths::new(one).within(&["/work".into()], &[]).unwrap();
        assert_eq!(reached(&paths, "/volume/work/x/../a"), Ok(()));
        let left = reached(&paths, "/volume/work/s/../../../work/a");
        assert_eq!(left, Err(Error::Outside));
        *working.borrow_mut() = PathBuf::from("/volume/work/s");
        assert_eq!(reached(&paths, "../a"), Ok(()));
        assert_eq!(reached(&paths, "x/../../a"), Ok(()));
        assert_eq!(reached(&paths, "../../work/a"), Ok(()));
        assert_eq!(reached(&paths, "../../../a"), Err(Error::Outside));
    }

    #[test]
    fn a_directory_on_the_way_with_no_key_gives_its_error() {
        let paths = Paths::new(One::default())
            .within(&["/".into()], &[])
            .unwrap();
        assert_eq!(reached(&paths, "/none/deep/a"), Err(Error::NotFound));
        assert_eq!(reached(&paths, "/work/new/a"), Err(Error::NotFound));
    }

    #[test]
    fn a_step_whose_key_fails_gives_its_error() {
        let paths = Paths::new(One::default())
            .within(&["/".into()], &[])
            .unwrap();
        assert_eq!(reached(&paths, "/work/io"), Err(Error::Io { code: 5 }));
    }

    #[test]
    fn a_path_stops_at_its_first_step_that_does_not_resolve() {
        let one = One::default();
        let calls = Rc::clone(&one.1);
        let paths = Paths::new(one).within(&["/".into()], &[]).unwrap();
        let mut path = String::from("/none/deep");
        for _ in 0..1000 {
            path.push_str("/a");
        }
        calls.set(0);
        assert_eq!(paths.read(Path::new(&path), 1), Err(Error::NotFound));
        assert_eq!(calls.get(), 7);
    }

    #[test]
    fn a_directory_is_not_made_past_a_step_with_an_error() {
        let free = Paths::new(One::default());
        free.write(Path::new("/a"), b"old").unwrap();
        let paths = free.within(&["/".into()], &[]).unwrap();
        let made = paths.make_dir(Path::new("/work/io/x"));
        assert_eq!(made, Err(Error::Io { code: 5 }));
        assert_eq!(
            free.read(Path::new("/a"), 3),
            Ok(Found::File(b"old".to_vec()))
        );
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
        assert_eq!(reached(&beside, "/work/data"), Err(Error::Outside));
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

    #[test]
    fn a_file_before_a_name_or_an_end_in_a_separator_gives_the_error_of_the_host() {
        let paths = Paths::new(One::default())
            .within(&["/work".into()], &[])
            .unwrap();
        paths.write(Path::new("/work/a.hcl"), b"x").unwrap();
        let file = Ok(Found::File(b"x".to_vec()));
        assert_eq!(paths.read(Path::new("/work/a.hcl"), 1), file);
        for path in [
            "/work/a.hcl/",
            "/work/a.hcl/.",
            "/work/a.hcl/..",
            "/work/a.hcl/x",
        ] {
            assert_eq!(reached(&paths, path), Err(Error::Io { code: 20 }), "{path}");
        }
    }

    #[test]
    fn a_walk_follows_40_links_and_past_them_gives_the_error_of_the_host() {
        let paths = Paths::new(One::default());
        let resolved = paths.resolve(Path::new("/work/c40"));
        assert_eq!(resolved, Ok(PathBuf::from("/work/c0")));
        let looped = Err(Error::Io { code: 40 });
        assert_eq!(paths.resolve(Path::new("/work/c41")), looped);
        paths.write(Path::new("/work/a"), b"loop").unwrap();
        assert_eq!(paths.resolve(Path::new("/work/loop")), looped);
    }

    #[test]
    fn make_dir_gives_replaced_for_a_loop_at_the_path_of_a_folder() {
        let paths = Paths::new(One::default());
        let within = paths.within(&["/work/loop".into()], &[]).unwrap();
        paths.write(Path::new("/work/a"), b"loop").unwrap();
        let made = within.make_dir(Path::new("/work/loop"));
        assert_eq!(made, Err(Error::Replaced));
    }

    #[test]
    fn make_dir_reaches_the_host_when_the_host_refuses_the_last_name() {
        let paths = Paths::new(One::default())
            .within(&["/work".into()], &[])
            .unwrap();
        let made = paths.make_dir(Path::new("/work/io"));
        assert_eq!(made, Err(Error::Io { code: 5 }));
        let file = Ok(Found::File(b"dir".to_vec()));
        assert_eq!(paths.read(Path::new("/work/a"), 3), file);
    }
}
