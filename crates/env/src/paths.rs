//! The files of the host by the paths that a user gave.

use std::fmt::Debug;
use std::path::{Component, Path, PathBuf, is_separator};
use std::rc::Rc;

use types::hash;
use walk::{Open, Step, Walk};

pub mod dir;
pub mod file;
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
    /// link target. The first step that fails gives its error:
    ///
    /// - [`Error::Outside`]: no folder of `folders` holds the step, by its file and not
    ///   by its name. The step where the path ends must be held. Another step is
    ///   checked only when a `..` takes the walk above it, nothing is there, or a
    ///   hidden folder holds it, and then a directory on the way to a folder also
    ///   passes.
    /// - [`Error::Replaced`]: in place of `Outside`, when the file at a folder is not
    ///   the one that was there at this call, and the step is under that folder by its
    ///   path then or now, is in the directory that it was, or, short of the end, is on
    ///   the way to it. A program that renames, replaces, or removes a folder does
    ///   this, unless the host gives the new folder the old key, as ext4 can. A write
    ///   through this handle replaces a folder that is a file too.
    /// - [`Error::Hidden`]: the step passes the folders and a hidden folder holds it.
    ///   Each call walks each path of `hidden` again, and a directory that was at a
    ///   hidden path at this call hides too, but a listing of it does not. A relative
    ///   path of `hidden` starts at the working directory of this call: its resolved
    ///   path, less one name for each `..` at the start of the path. So a later change
    ///   or removal of the working directory moves no hidden path. When the host
    ///   refuses that walk, other than for a name that is not there, a step that is a
    ///   file, or a loop, each step that passes the folders is hidden. A `..` that
    ///   finds no directory above is such a refusal, also when a name of the path that
    ///   the host gives for that directory is not there, is a file, or loops.
    /// - [`Error::Vacant`]: while nothing is at a hidden path, a write or a `make_dir`
    ///   of a new name in the deepest directory on the way to it.
    ///
    /// When the steps pass and the walk fails, the call gives the error of the walk.
    /// Each call runs on the directories that its walk opened, so a link that a
    /// program puts in a folder after the walk reaches no file.
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
            let given = self.on_working(given)?;
            let end = Walk::on_root(&*self.driver, &given).into_end()?;
            limit.hidden.push(Hidden {
                given,
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
    /// also at a `..` whose directory a program moves or removes during the walk, the
    /// error of [`Refusal::Looped`] past [`LINKS`] symbolic links, else
    /// [`Error::Io`], also for a `.`, a `..`, or an end in `/` after a file, and for a
    /// `path` longer than the host takes. Each name goes to the host as one call on
    /// the directory that holds it, so a resolved path has no bound on its length.
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
    /// `path` does not change what it reads. A directory that this user can read and
    /// not search gives its entries, and each name in it gives `EACCES`. It follows a
    /// symbolic link by its text, so a link of Linux such as `/proc/<pid>/fd/N` or
    /// `/dev/fd/N` to a removed file, a removed directory, or a pipe names nothing.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing is at `path`, [`Error::Outside`],
    /// [`Error::Hidden`], or [`Error::Replaced`] past the limits of [`Paths::within`],
    /// [`Error::Io`] 22 when `path` is a file that is neither a regular file nor a
    /// directory, such as a FIFO or a device, [`Error::Io`] 13 when `path` is a
    /// directory that this user can search and not read, else [`Error::Io`].
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
        let mut walk = self.check(path, Call::Read)?;
        walk.end??;
        let reached = walk.steps.last().is_some_and(|step| step.own.is_some());
        match walk.open.take() {
            Some(Open::Directory(dir)) => list(path, &*dir, most),
            Some(Open::In { special: true, .. }) => {
                Err(self.driver.error(Refusal::Irregular))
            }
            Some(Open::In { holder, name, .. }) if reached => {
                let opened = holder.open(&name)?;
                self.again(&mut walk, opened.key(), Call::Read)?;
                match opened {
                    dir::Opened::Directory(dir) => list(path, &*dir, most),
                    dir::Opened::File(file) => Ok(file
                        .read(most)?
                        .map_or(Found::Large(Kind::File), Found::File)),
                }
            }
            _ => Err(Error::NotFound),
        }
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
    /// past the limits of [`Paths::within`], [`Error::Io`] 21 when a directory is at
    /// `path`, else [`Error::Io`].
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
        let mut walk = self.check(path, Call::Make)?;
        walk.end??;
        let reached = walk.steps.last().is_some_and(|step| step.own.is_some());
        match walk.open.take() {
            Some(Open::Directory(_)) => Err(self.driver.error(Refusal::Directory)),
            Some(Open::In {
                holder,
                name,
                special,
            }) if reached && (special || walk.linked) => {
                let file = holder.file(&name)?;
                self.again(&mut walk, file.key(), Call::Make)?;
                file.write(bytes)
            }
            // A path that ends in `/` or `.` names no new file.
            Some(Open::In { holder, name, .. })
                if !walk.linked && (reached || !names_directory(path)) =>
            {
                holder.replace(&name, bytes)
            }
            _ => Err(Error::NotFound),
        }
    }

    /// Makes the directory at `path`. Gives `Ok` when a directory is there already. It
    /// makes no directory above it, and none through a symbolic link at `path`. A
    /// directory that it makes gives the group and other users no access. On a handle
    /// of [`Paths::within`], `path` first passes the limits of [`Paths::within`]. When
    /// the host refuses the last name of `path` to the walk, such as for a name longer
    /// than the host takes, the limits check that name at its place as a new name, and
    /// the call gives the result of the host.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the directory above `path` is not there,
    /// [`Error::Outside`], [`Error::Hidden`], [`Error::Vacant`], or [`Error::Replaced`]
    /// past the limits of [`Paths::within`], [`Error::Io`] 17 when a file, or a
    /// symbolic link to no directory, is at `path`, else [`Error::Io`].
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
        let walk = self.check(path, Call::Make)?;
        let directory =
            walk.end == Ok(Ok(())) && matches!(walk.open, Some(Open::Directory(_)));
        if dotted(path) {
            walk.end??;
            return if directory {
                Ok(())
            } else {
                Err(Error::NotFound)
            };
        }
        if let (Err(e), false) = (walk.end, walk.linked) {
            return Err(e);
        }
        if directory {
            return Ok(());
        }
        // The link can go after the walk, so a call could make a directory beside it.
        if walk.linked {
            return Err(self.driver.error(Refusal::Taken));
        }
        match walk.open {
            // The host takes the last name with no link followed, so its result stands
            // for an error of the walk at that name.
            Some(Open::In { holder, name, .. }) => holder.make_dir(&name),
            _ => Err(walk
                .end?
                .expect_err("invariant: a walk that holds no directory failed")),
        }
    }

    /// The walk of `path` when each step of it passes each limit for `call`, else the
    /// error of the first step that fails.
    fn check(&self, path: &Path, call: Call) -> Result<Walk, Error> {
        let walk = self.walk(path);
        if self.limits.is_empty() {
            return Ok(walk);
        }
        let now: Vec<_> = self.limits.iter().map(|limit| self.now(limit)).collect();
        let above = walk::Above::new(&walk.steps);
        for (index, step) in walk.steps.iter().enumerate() {
            let ask = if index + 1 == walk.steps.len() && walk.end.is_ok() {
                Ask::Hold(call)
            } else if step.back || step.own.is_none() {
                Ask::Way
            } else {
                Ask::Pass
            };
            self.allowed(&walk.steps, index, ask, &now, &above)?;
        }
        Ok(walk)
    }

    /// Checks the last step of `walk`, which reached its end, again for `call`, when
    /// the file that a call opened there has another `key`: a program put another file
    /// at its name after the walk.
    fn again(&self, walk: &mut Walk, key: Key, call: Call) -> Result<(), Error> {
        let index = walk.steps.len() - 1;
        if walk.steps[index].own == Some(key) {
            return Ok(());
        }
        walk.steps[index].own = Some(key);
        let now: Vec<_> = self.limits.iter().map(|limit| self.now(limit)).collect();
        let above = walk::Above::new(&walk.steps);
        self.allowed(&walk.steps, index, Ask::Hold(call), &now, &above)
    }

    /// `Ok` when step `index` of `steps` passes each limit as `ask` gives, where `now`
    /// is what this call found of each limit, and `above` what the directories above
    /// each step of `steps` hold.
    fn allowed(
        &self,
        steps: &[Step],
        index: usize,
        ask: Ask,
        now: &[Now],
        above: &walk::Above<'_>,
    ) -> Result<(), Error> {
        let own = steps[index].own;
        let has = |key: Key| own == Some(key) || above.holds(index, key);
        let (mut vacant, mut veiled) = (false, false);
        for (limit, now) in self.limits.iter().zip(now) {
            for (Hidden { was, .. }, seen) in limit.hidden.iter().zip(&now.hidden) {
                let moved = was.is_some_and(|key| above.holds(index, key));
                let hidden = match seen {
                    Seen::There(key) => has(*key),
                    Seen::Vacant(deepest) => {
                        vacant |=
                            own.is_none() && walk::holder(steps, index) == *deepest;
                        false
                    }
                    Seen::Refused => true,
                    Seen::Unreached => false,
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
                && (has(*was)
                    || ask == Ask::Way
                        && own.is_some_and(|own| folder.above.contains(&own)))
        };
        if ask != Ask::Pass
            && let Some((limit, _)) =
                self.limits.iter().zip(now).find(|(limit, now)| {
                    !limit.folders.iter().zip(&now.folders).any(passes)
                })
        {
            let path = walk::path(steps, index);
            let replaced = limit.folders.iter().any(|(dir, was)| {
                has(*was)
                    || path.starts_with(dir)
                    || !matches!(ask, Ask::Hold(_)) && dir.starts_with(&path)
                    || self.walk(dir).into_end().is_ok_and(|to| {
                        // A host that ignores case reaches the folder by a name
                        // in other case, which only the key tells.
                        path.starts_with(&to.path) || to.own.is_some_and(has)
                    })
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
        let folders = limit.folders.iter().map(|(dir, was)| {
            let (above, own) = walk::keys(&*self.driver, dir);
            Folder {
                there: own == Some(*was),
                above,
            }
        });
        let hidden = limit.hidden.iter().map(|Hidden { given, .. }| {
            let walk = Walk::on_root(&*self.driver, given);
            match (&walk.end, walk.steps.last()) {
                _ if walk.lost => Seen::Refused,
                (Ok(Ok(())), Some(Step { own: Some(key), .. })) => Seen::There(*key),
                (Ok(Ok(())) | Err(Error::NotFound), Some(Step { own: None, .. })) => {
                    Seen::Vacant(walk::holder(&walk.steps, walk.steps.len() - 1))
                }
                (Ok(Err(error)) | Err(error), _)
                    if !walk.in_file
                        && *error != self.driver.error(Refusal::Looped) =>
                {
                    Seen::Refused
                }
                _ => Seen::Unreached,
            }
        });
        Now {
            folders: folders.collect(),
            hidden: hidden.collect(),
        }
    }

    /// `path` on the resolved working directory when it is relative, less one name for
    /// each `..` at its start: the directory has no link on it, so the host takes each
    /// such `..` by text. Gives the error of [`Driver::absolute`] for `path` as given,
    /// which bounds its length.
    fn on_working(&self, path: &Path) -> Result<PathBuf, Error> {
        if path.as_os_str().is_empty() {
            return Ok(PathBuf::new());
        }
        self.driver.absolute(path)?;
        if path.is_absolute() {
            return Ok(path.to_owned());
        }
        let mut at = self.resolve(Path::new("."))?;
        let mut parts = path.components().peekable();
        while let Some(part) = parts
            .next_if(|part| matches!(part, Component::CurDir | Component::ParentDir))
        {
            if part == Component::ParentDir {
                at.pop();
            }
        }
        at.extend(parts);
        if names_directory(path) {
            at.push("");
        }
        Ok(at)
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
    /// The path that the caller gave, on the working directory of `within` when it is
    /// relative, which each call walks again.
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
    /// The key of each directory on the way to its resolved path, with no link
    /// followed.
    above: hash::Set<Key>,
}

/// What one call finds at a hidden path.
enum Seen {
    There(Key),
    /// Nothing, with the key of the deepest directory on the way that is there.
    Vacant(Option<Key>),
    /// The host refused the walk other than for a name that is not there, a step that
    /// is a file, or a loop, or the walk lost its place after a `..` with no step
    /// above. The file there can have another name, so each step is hidden.
    Refused,
    /// The host reaches no file by the path: a name on the way is not there, a step on
    /// the way is a file, or the walk passes [`LINKS`] links.
    Unreached,
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
    /// The home directory of the user as the host gives it, or `None` when it gives
    /// none.
    fn home(&self) -> Option<PathBuf>;

    /// The root directory of the host on the way to `absolute`, a path with a root,
    /// open: `/` on a host other than Windows.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] with the code of the host.
    fn root(&self, absolute: &Path) -> Result<Box<dyn dir::Driver>, Error>;

    /// The absolute path that the host walks for `path`, as text: a relative path on
    /// the resolved working directory, and on Windows the path that Win32 makes of it.
    /// It reads no file other than the working directory. On a host other than
    /// Windows, it keeps each `..`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the working directory is gone, else [`Error::Io`], also
    /// when `path` is longer than the host takes.
    fn absolute(&self, path: &Path) -> Result<PathBuf, Error>;

    /// The error that the host gives for `refusal`.
    fn error(&self, refusal: Refusal) -> Error;
}

/// A refusal of the host that [`Paths`] gives with no call to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A name in a file that is not a directory: `ENOTDIR` on Linux and macOS.
    NotDirectory,
    /// A write of a directory: `EISDIR`.
    Directory,
    /// A directory made at a name that a file or a link holds: `EEXIST`.
    Taken,
    /// A path through more than [`LINKS`] symbolic links: `ELOOP`.
    Looped,
    /// A read of a file that is neither a regular file nor a directory: `EINVAL`.
    Irregular,
}

/// The entries of `dir`, the directory at `path`, as [`Paths::read`] gives them.
fn list(path: &Path, dir: &dyn dir::Driver, most: usize) -> Result<Found, Error> {
    let large = Ok(Found::Large(Kind::Directory));
    let Some(names) = dir.names(most)? else {
        return large;
    };
    let (mut entries, mut size) = (Vec::with_capacity(names.len()), 0_usize);
    for name in names {
        let entry = path.join(name);
        size = size.saturating_add(entry.as_os_str().len());
        if size > most {
            return large;
        }
        entries.push(entry);
    }
    entries.sort_unstable();
    Ok(Found::Directory(entries))
}

/// Whether the last name of `path`, with no end separator, is `.` or `..`.
fn dotted(path: &Path) -> bool {
    let bytes = path.as_os_str().as_encoded_bytes();
    let name = bytes
        .rsplit(|&byte| is_separator(char::from(byte)))
        .find(|name| !name.is_empty());
    matches!(name, Some(b"." | b".."))
}

/// The most symbolic links that the walk of a path goes through, as on Linux. A `..`
/// to a directory that is no step of the walk, such as `/usr` above
/// `/System/Volumes/Data/usr/local` on macOS, takes one of them. A host can take
/// fewer.
pub const LINKS: usize = 40;

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
    use std::ffi::{OsStr, OsString};

    use super::dir::Entry;
    use super::*;

    /// The length of a path that the fake host refuses as given. It is far under the
    /// 4096 of Linux, so that the tests of the bound stay short under Miri.
    const MOST: usize = 64;

    /// One file, at any path but one under `/none`, where each other name is a
    /// directory, other than a name that ends in `.hcl`, which is a file. After a write
    /// of `loop`, the name `loop` is a link to itself. `c<n>` for n from 1 is a link to
    /// `c<n - 1>`, `link` a link to `/out`, `jump` a link to `moving.hcl`, and `stray`
    /// a link to `new`. `fifo` is a special file. `/none/deep` is a link to
    /// `/none/new/a`. A directory that it makes is the file `dir`. A name in `/none` is
    /// there only while the last write made a directory, and a name that starts with
    /// `new` never. The key of a path ignores case. The name `Gone`, in that case,
    /// gives `Io` 13 after a write of `gone`. The key of a name that starts with `swap`
    /// in any case takes the bytes of the file after the path, so a write gives it a
    /// new key: after a write of `x`, `/work/swap` has the key that `/work/swapx` had
    /// before it. The name `io` gives `Io` 5. After a write of `file`, the name `f` is
    /// a file. After a write of `hop`, `/hop` is a link to `/out`. A path in `/volume`
    /// has the key of the path without `/volume`, as a second mount of `/` does. `..`
    /// of a directory is its path less its last name, but `..` of `moved` is `/out`
    /// after a write of `moved`, `away`, `onto`, `vanished`, `through`, `circle`, or
    /// `full`, whose path the host gives as `/out`, `/work`, `/work/a.hcl`,
    /// `/work/new`, `/work/a.hcl/x`, `/work/c41`, or `Io` 24. `..` of `firm` is `/`, as
    /// macOS gives `/` above the target of a firmlink, and `..` of `off` is `/usr`. An
    /// open of `moving.hcl` to read finds the directory `/work/data`, and to write the
    /// file `/work/a.hcl`, as after a rename over it.
    /// It counts its opens of a root, a name, or a file, and a relative path is under
    /// its working directory, `/` at first. `absolute` refuses a path of [`MOST`] bytes
    /// or more as given with `Io` 36. A `\` in a path is a `/`, since the walk joins
    /// names with `\` on Windows.
    #[derive(Clone, Debug, Default)]
    struct One(Rc<RefCell<Vec<u8>>>, Rc<Cell<usize>>, Rc<RefCell<PathBuf>>);

    impl One {
        fn count(&self) {
            self.1.set(self.1.get() + 1);
        }

        /// Puts `bytes` in the file, or gives `Io` 28 for none.
        fn set(&self, bytes: &[u8]) -> Result<(), Error> {
            if bytes.is_empty() {
                return Err(Error::Io { code: 28 });
            }
            *self.0.borrow_mut() = bytes.to_vec();
            Ok(())
        }

        fn key(&self, path: &Path) -> Key {
            let name = path.file_name().unwrap_or_default().as_encoded_bytes();
            let mut at = path.to_path_buf();
            if let Ok(rest) = path.strip_prefix("/volume")
                && rest != Path::new("")
            {
                at = Path::new("/").join(rest);
            }
            let mut folded = at.as_os_str().as_encoded_bytes().to_ascii_lowercase();
            if name.to_ascii_lowercase().starts_with(b"swap") {
                folded.extend_from_slice(&self.0.borrow());
            }
            // FNV-1a.
            let hash = folded
                .iter()
                .fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
                    (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
                });
            Key::new(0, hash)
        }

        fn entry(&self, path: &Path) -> Result<Entry, Error> {
            let data = self.0.borrow();
            let name = path.file_name().unwrap_or_default().as_encoded_bytes();
            if name == b"io" {
                return Err(Error::Io { code: 5 });
            }
            if name == b"Gone" && *data == b"gone" {
                return Err(Error::Io { code: 13 });
            }
            let in_none = path.parent() == Some(Path::new("/none")) && *data != b"dir";
            if name.starts_with(b"new") || in_none && path != Path::new("/none/deep") {
                return Err(Error::NotFound);
            }
            let key = self.key(path);
            Ok(match path.to_str() {
                Some("/none/deep") => Entry::Link(key, "/none/new/a".into()),
                Some("/hop") if *data == b"hop" => Entry::Link(key, "/out".into()),
                Some("/lifted") if *data == b"lifted" => {
                    Entry::File(self.key(Path::new("/lifted/a.hcl")))
                }
                _ if name == b"link" => Entry::Link(key, "/out".into()),
                _ if name == b"jump" => Entry::Link(key, "moving.hcl".into()),
                _ if name == b"stray" => Entry::Link(key, "new".into()),
                _ if name == b"fifo" => Entry::Special(key),
                _ if name == b"loop" && *data == b"loop" => {
                    Entry::Link(key, "loop".into())
                }
                _ if name.ends_with(b".hcl") || name == b"f" && *data == b"file" => {
                    Entry::File(key)
                }
                _ => match name
                    .strip_prefix(b"c")
                    .and_then(|n| str::from_utf8(n).ok()?.parse::<u8>().ok())
                {
                    Some(n @ 1..) => Entry::Link(key, format!("c{}", n - 1).into()),
                    _ => Entry::Directory(Box::new(Dir::at(self, path.into()))),
                },
            })
        }
    }

    /// A directory of [`One`], open at its path, with the path that the host gives.
    #[derive(Debug)]
    struct Dir(One, PathBuf, Result<PathBuf, Error>);

    impl Dir {
        fn at(one: &One, path: PathBuf) -> Self {
            Self(one.clone(), path.clone(), Ok(path))
        }
    }

    impl dir::Driver for Dir {
        fn key(&self) -> Key {
            self.0.key(&self.1)
        }

        fn entry(&self, name: &OsStr) -> Result<Entry, Error> {
            self.0.count();
            self.0.entry(&slashed(&self.1.join(name)))
        }

        fn up(&self) -> Result<Box<dyn dir::Driver>, Error> {
            self.0.count();
            let moved = self.1.file_name() == Some(OsStr::new("moved"));
            let given = match &self.0.0.borrow()[..] {
                _ if !moved => None,
                b"moved" => Some(Ok(PathBuf::from("/out"))),
                b"away" => Some(Ok(PathBuf::from("/work"))),
                b"onto" => Some(Ok(PathBuf::from("/work/a.hcl"))),
                b"vanished" => Some(Ok(PathBuf::from("/work/new"))),
                b"through" => Some(Ok(PathBuf::from("/work/a.hcl/x"))),
                b"circle" => Some(Ok(PathBuf::from("/work/c41"))),
                b"full" => Some(Err(Error::Io { code: 24 })),
                _ => None,
            };
            if let Some(given) = given {
                let out = PathBuf::from("/out");
                return Ok(Box::new(Dir(self.0.clone(), out, given)));
            }
            let up = match self.1.file_name().and_then(OsStr::to_str) {
                Some("firm") => PathBuf::from("/"),
                Some("off") => PathBuf::from("/usr"),
                _ => self.1.parent().unwrap_or(&self.1).to_owned(),
            };
            Ok(Box::new(Dir::at(&self.0, up)))
        }

        fn path(&self) -> Result<PathBuf, Error> {
            self.2.clone()
        }

        fn names(&self, most: usize) -> Result<Option<Vec<OsString>>, Error> {
            let names = ["b", "a.hcl"].map(OsString::from);
            let size: usize = names.iter().map(|name| name.len()).sum();
            Ok((size <= most).then(|| names.to_vec()))
        }

        fn open(&self, name: &OsStr) -> Result<dir::Opened, Error> {
            self.0.count();
            let file = |key| dir::Opened::File(Box::new(File(self.0.clone(), key)));
            Ok(match self.0.entry(&slashed(&self.1.join(name)))? {
                _ if name == "moving.hcl" => {
                    let data = Dir::at(&self.0, PathBuf::from("/work/data"));
                    dir::Opened::Directory(Box::new(data))
                }
                Entry::File(key) | Entry::Special(key) => file(key),
                Entry::Link(..) => return Err(Error::Io { code: 40 }),
                Entry::Directory(dir) => dir::Opened::Directory(dir),
            })
        }

        fn file(&self, name: &OsStr) -> Result<Box<dyn file::Driver>, Error> {
            self.0.count();
            let key = match self.0.entry(&slashed(&self.1.join(name)))? {
                _ if name == "moving.hcl" => self.0.key(Path::new("/work/a.hcl")),
                Entry::File(key) | Entry::Special(key) => key,
                Entry::Link(..) => return Err(Error::Io { code: 40 }),
                Entry::Directory(_) => return Err(Error::Io { code: 21 }),
            };
            Ok(Box::new(File(self.0.clone(), key)))
        }

        fn replace(&self, _: &OsStr, bytes: &[u8]) -> Result<(), Error> {
            self.0.set(bytes)
        }

        fn make_dir(&self, _: &OsStr) -> Result<(), Error> {
            self.0.set(b"dir")
        }
    }

    impl Driver for One {
        fn home(&self) -> Option<PathBuf> {
            Some(PathBuf::from("/home/user"))
        }

        fn root(&self, _: &Path) -> Result<Box<dyn dir::Driver>, Error> {
            self.count();
            Ok(Box::new(Dir::at(self, PathBuf::from("/"))))
        }

        fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
            if path.as_os_str().len() >= MOST {
                return Err(Error::Io { code: 36 });
            }
            Ok(Path::new("/").join(&*self.2.borrow()).join(path))
        }

        fn error(&self, refusal: Refusal) -> Error {
            let code = match refusal {
                Refusal::NotDirectory => 20,
                Refusal::Directory => 21,
                Refusal::Taken => 17,
                Refusal::Looped => 40,
                Refusal::Irregular => 22,
            };
            Error::Io { code }
        }
    }

    /// A file of [`One`], open, with the key of its open.
    #[derive(Debug)]
    struct File(One, Key);

    impl file::Driver for File {
        fn key(&self) -> Key {
            self.1
        }

        fn read(&self, most: usize) -> Result<Option<Vec<u8>>, Error> {
            let bytes = self.0.0.borrow();
            Ok((bytes.len() <= most).then(|| bytes.clone()))
        }

        fn write(&self, bytes: &[u8]) -> Result<(), Error> {
            self.0.set(bytes)
        }
    }

    fn slashed(path: &Path) -> PathBuf {
        path.to_string_lossy().replace('\\', "/").into()
    }

    /// What a write and a `make_dir` of `path` give, which agree, where a write of a
    /// directory gives `Ok`. A read gives the same, but passes the limits where they
    /// give `Vacant`, and finds nothing at a new name.
    fn reached(paths: &Paths, path: &str) -> Result<(), Error> {
        let path = Path::new(path);
        let written = match paths.write(path, b"x") {
            Err(Error::Io { code: 21 }) => Ok(()),
            written => written,
        };
        let read = match paths.read(path, 1) {
            Err(Error::NotFound) if matches!(written, Ok(()) | Err(Error::Vacant)) => {
                Ok(())
            }
            read => read.map(|_| ()),
        };
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
        assert_eq!(paths.write(Path::new("a.hcl"), b"xyz"), Ok(()));
        assert_eq!(
            clone.read(Path::new("a.hcl"), 3),
            Ok(Found::File(b"xyz".to_vec()))
        );
        let large = Ok(Found::Large(Kind::File));
        assert_eq!(clone.read(Path::new("a.hcl"), 2), large);
        let entries = ["dir/a.hcl", "dir/b"].map(PathBuf::from).to_vec();
        let read = paths.read(Path::new("dir"), 64);
        assert_eq!(read, Ok(Found::Directory(entries)));
        assert_eq!(paths.read(Path::new(""), 0), Err(Error::NotFound));
        assert_eq!(
            paths.write(Path::new("a.hcl"), b""),
            Err(Error::Io { code: 28 })
        );
        assert_eq!(paths.make_dir(Path::new("new")), Ok(()));
        assert_eq!(
            paths.read(Path::new("a.hcl"), 3),
            Ok(Found::File(b"dir".to_vec()))
        );
    }

    /// The opens of a root, a name, or a file that `call` makes past those of the walk
    /// of `path`, on the host of `paths`.
    fn opened(one: &One, path: &str, call: impl FnOnce(&Paths, &Path)) -> usize {
        let (paths, path) = (Paths::new(one.clone()), Path::new(path));
        one.1.set(0);
        drop(paths.resolve(path));
        let walk = one.1.get();
        one.1.set(0);
        call(&paths, path);
        one.1.get() - walk
    }

    #[test]
    fn a_read_gives_the_entries_of_a_directory_in_order_bounded_by_their_paths() {
        let paths = Paths::new(One::default());
        let entries = ["/work/a.hcl", "/work/b"].map(PathBuf::from).to_vec();
        let read = paths.read(Path::new("/work"), 18);
        assert_eq!(read, Ok(Found::Directory(entries)));
        let large = Ok(Found::Large(Kind::Directory));
        assert_eq!(paths.read(Path::new("/work"), 17), large);
        assert_eq!(paths.read(Path::new("/work"), 5), large);
    }

    #[test]
    fn a_read_of_a_special_file_or_a_new_name_opens_nothing() {
        let one = One::default();
        let opens = opened(&one, "/work/fifo", |paths, path| {
            assert_eq!(paths.read(path, 8), Err(Error::Io { code: 22 }));
        });
        assert_eq!(opens, 0);
        let opens = opened(&one, "/work/new", |paths, path| {
            assert_eq!(paths.read(path, 8), Err(Error::NotFound));
        });
        assert_eq!(opens, 0);
        assert_eq!(
            opened(&one, "/work/a.hcl", |paths, path| {
                paths.read(path, 8).unwrap();
            }),
            1
        );
    }

    #[test]
    fn a_write_takes_the_bytes_in_place_at_a_link_or_a_special_file() {
        let one = One::default();
        for path in ["/work/jump", "/work/fifo"] {
            let opens = opened(&one, path, |paths, path| {
                assert_eq!(
                    paths.write(path, path.as_os_str().as_encoded_bytes()),
                    Ok(())
                );
            });
            assert_eq!(opens, 1, "{path}");
            assert_eq!(one.0.borrow().as_slice(), path.as_bytes());
        }
        let opens = opened(&one, "/work/a.hcl", |paths, path| {
            assert_eq!(paths.write(path, b"new"), Ok(()));
        });
        assert_eq!((opens, one.0.borrow().as_slice()), (0, &b"new"[..]));
    }

    #[test]
    fn a_write_of_a_directory_a_link_to_nothing_or_a_new_name_as_a_directory_fails() {
        let one = One::default();
        let paths = Paths::new(one.clone());
        paths.write(Path::new("/a.hcl"), b"old").unwrap();
        for (path, error) in [
            ("/work/b", Error::Io { code: 21 }),
            ("/work/stray", Error::NotFound),
            ("/work/new/", Error::NotFound),
            ("/work/new/.", Error::NotFound),
        ] {
            assert_eq!(paths.write(Path::new(path), b"x"), Err(error), "{path}");
        }
        assert_eq!(one.0.borrow().as_slice(), b"old");
        assert_eq!(paths.write(Path::new("/work/new"), b"x"), Ok(()));
    }

    #[test]
    fn a_call_checks_a_file_that_its_open_finds_in_place_of_the_walk_again() {
        let free = Paths::new(One::default());
        let hidden = ["/work/data".into(), "/work/a.hcl".into()];
        let paths = free.within(&["/work".into()], &hidden).unwrap();
        let moving = Path::new("/work/moving.hcl");
        assert_eq!(paths.read(moving, 8), Err(Error::Hidden));
        assert_eq!(
            paths.write(Path::new("/work/jump"), b"x"),
            Err(Error::Hidden)
        );
        let entries =
            ["/work/moving.hcl/a.hcl", "/work/moving.hcl/b"].map(PathBuf::from);
        let listed = free.read(moving, 64);
        assert_eq!(listed, Ok(Found::Directory(entries.to_vec())));
        // A rename over the name leaves the file that the open found.
        assert_eq!(paths.write(moving, b"x"), Ok(()));
    }

    #[test]
    fn make_dir_makes_no_call_at_a_dot_a_link_or_a_directory() {
        let one = One::default();
        let paths = Paths::new(one.clone());
        paths.write(Path::new("/a.hcl"), b"old").unwrap();
        for (path, made) in [
            ("/work/a.hcl/..", Err(Error::Io { code: 20 })),
            ("/work/new/.", Err(Error::NotFound)),
            ("/work/new/./", Err(Error::NotFound)),
            ("/work/new/..", Err(Error::NotFound)),
            ("/work/b/..", Ok(())),
            ("/work/.", Ok(())),
            ("/work/b", Ok(())),
            ("/work/jump", Err(Error::Io { code: 17 })),
            ("/work/stray", Err(Error::Io { code: 17 })),
            ("/work/stray/", Err(Error::Io { code: 17 })),
        ] {
            assert_eq!(paths.make_dir(Path::new(path)), made, "{path}");
        }
        assert_eq!(one.0.borrow().as_slice(), b"old");
        assert_eq!(paths.make_dir(Path::new("/work/new/")), Ok(()));
        assert_eq!(one.0.borrow().as_slice(), b"dir");
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
        free.write(Path::new("/x.hcl"), b"gone").unwrap();
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
            Err(Error::Io { code: 17 })
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
        let paths = Paths::new(One::default())
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
        free.write(Path::new("/work/a.hcl"), b"y").unwrap();
        assert_eq!(reached(&paths, "/WORK/SWAP"), Err(Error::Hidden));
    }

    #[test]
    fn a_hidden_folder_that_moves_hides_the_files_under_it_but_not_itself() {
        let free = Paths::new(One::default());
        let paths = free
            .within(&["/work".into()], &["/work/swapx".into()])
            .unwrap();
        free.write(Path::new("/work/a.hcl"), b"x").unwrap();
        assert_eq!(reached(&paths, "/work/swap/a"), Err(Error::Hidden));
        let entries = ["/work/swap/a.hcl", "/work/swap/b"].map(PathBuf::from);
        assert_eq!(
            paths.read(Path::new("/work/swap"), 64),
            Ok(Found::Directory(entries.to_vec()))
        );
    }

    #[test]
    fn a_folder_that_is_replaced_holds_nothing() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/work/swapx".into()], &[]).unwrap();
        free.write(Path::new("/work/a.hcl"), b"x").unwrap();
        assert_eq!(reached(&paths, "/work/swap/a"), Err(Error::Replaced));
        assert_eq!(reached(&paths, "/work/swapx/a"), Err(Error::Replaced));
    }

    #[test]
    fn a_folder_that_is_replaced_is_replaced_by_a_name_in_other_case() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/work/swapx".into()], &[]).unwrap();
        free.write(Path::new("/work/a.hcl"), b"x").unwrap();
        assert_eq!(reached(&paths, "/work/SWAPX/a"), Err(Error::Replaced));
    }

    #[test]
    fn a_folder_that_a_link_replaces_holds_nothing() {
        let free = Paths::new(One::default());
        let paths = free.within(&["/hop".into()], &[]).unwrap();
        free.write(Path::new("/x.hcl"), b"hop").unwrap();
        let read = paths.read(Path::new("/hop/a"), 3).map(|_| ());
        assert_eq!(read, Err(Error::Replaced));
    }

    #[test]
    fn a_folder_that_is_gone_is_replaced_at_its_path() {
        let free = Paths::new(One::default());
        free.make_dir(Path::new("/none/gone")).unwrap();
        let paths = free.within(&["/none/gone".into()], &[]).unwrap();
        free.write(Path::new("/a.hcl"), b"x").unwrap();
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
    fn a_folder_that_is_a_file_holds_its_file() {
        let paths = Paths::new(One::default())
            .within(&["/a.hcl".into()], &[])
            .unwrap();
        assert_eq!(reached(&paths, "/a.hcl"), Ok(()));
    }

    #[test]
    fn a_folder_that_is_a_file_is_replaced_when_it_moves_above_its_path() {
        let one = One::default();
        let data = Rc::clone(&one.0);
        let paths = Paths::new(one)
            .within(&["/lifted/a.hcl".into()], &[])
            .unwrap();
        *data.borrow_mut() = b"lifted".to_vec();
        assert_eq!(paths.read(Path::new("/lifted"), 8), Err(Error::Replaced));
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
    fn a_relative_hidden_path_starts_at_the_working_directory_of_within() {
        let one = One::default();
        let working = Rc::clone(&one.2);
        *working.borrow_mut() = PathBuf::from("/work/s");
        let hidden = ["../data".into(), "./h".into()];
        let paths = Paths::new(one).within(&["/work".into()], &hidden).unwrap();
        *working.borrow_mut() = PathBuf::from("/");
        let got = ["/work/data/k", "/work/s/h/k", "/work/s/data/k", "/work/h/k"]
            .map(|path| reached(&paths, path));
        let hidden = Err(Error::Hidden);
        assert_eq!(got, [hidden, hidden, Ok(()), Ok(())]);
    }

    /// The fake host bounds a path as given, and the joined path is longer.
    #[test]
    fn a_short_relative_hidden_path_from_a_deep_working_directory_hides() {
        let one = One::default();
        let deep = format!("/work/{}", "d".repeat(MOST - 10));
        *one.2.borrow_mut() = PathBuf::from(&deep);
        let paths = Paths::new(one)
            .within(&["/work".into()], &["s/../k".into()])
            .unwrap();
        assert_eq!(reached(&paths, &format!("{deep}/k")), Err(Error::Hidden));
        assert_eq!(reached(&paths, &format!("{deep}/s")), Ok(()));
    }

    #[test]
    fn a_relative_hidden_path_longer_than_the_host_takes_fails_within() {
        let one = One::default();
        *one.2.borrow_mut() = PathBuf::from("/work/s");
        let hidden = PathBuf::from(format!("{}k", "../".repeat(MOST / 3)));
        let within = Paths::new(one).within(&["/".into()], &[hidden]);
        assert_eq!(within.err(), Some(Error::Io { code: 36 }));
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
        let path = Path::new("/work/io");
        let io = Err(Error::Io { code: 5 });
        let got = (paths.write(path, b"x"), paths.read(path, 1).map(|_| ()));
        assert_eq!(got, (io, io));
    }

    #[test]
    fn a_path_stops_at_its_first_step_that_does_not_resolve() {
        let one = One::default();
        let calls = Rc::clone(&one.1);
        let paths = Paths::new(one).within(&["/".into()], &[]).unwrap();
        let mut path = String::from("/none/deep");
        for _ in 0..25 {
            path.push_str("/a");
        }
        calls.set(0);
        assert_eq!(paths.read(Path::new(&path), 1), Err(Error::NotFound));
        assert_eq!(calls.get(), 7);
    }

    #[test]
    fn a_dot_dot_above_a_moved_directory_follows_the_path_that_the_host_gives() {
        let one = One::default();
        let calls = Rc::clone(&one.1);
        let paths = Paths::new(one);
        paths.write(Path::new("/a.hcl"), b"moved").unwrap();
        calls.set(0);
        let resolved = paths.resolve(Path::new("/work/moved/../a"));
        assert_eq!(resolved, Ok(PathBuf::from("/out/a")));
        assert_eq!(calls.get(), 7);
    }

    #[test]
    fn a_dot_dot_of_moved_with_no_write_is_its_parent() {
        let paths = Paths::new(One::default());
        let resolved = paths.resolve(Path::new("/work/moved/../a"));
        assert_eq!(resolved, Ok(PathBuf::from("/work/a")));
    }

    #[test]
    fn a_dot_dot_whose_host_path_ends_at_another_directory_is_not_found() {
        let paths = Paths::new(One::default());
        paths.write(Path::new("/a.hcl"), b"away").unwrap();
        for path in ["/work/moved/..", "/work/moved/../a"] {
            assert_eq!(
                paths.resolve(Path::new(path)),
                Err(Error::NotFound),
                "{path}"
            );
        }
    }

    #[test]
    fn an_error_after_a_dot_dot_with_no_step_above_ends_no_step() {
        let one = One::default();
        let limited = Paths::new(one.clone())
            .within(&["/work".into()], &[])
            .unwrap();
        for (path, data, error) in [
            ("/work/moved/..", &b"full"[..], Error::Io { code: 24 }),
            ("/work/moved/..", b"away", Error::NotFound),
            ("/work/moved/..", b"onto", Error::NotFound),
            ("/work/moved/..", b"vanished", Error::NotFound),
            ("/work/moved/..", b"through", Error::Io { code: 20 }),
            ("/work/moved/..", b"circle", Error::Io { code: 40 }),
            ("/work/c40/moved/..", b"away", Error::Io { code: 40 }),
        ] {
            Paths::new(one.clone())
                .write(Path::new("/a.hcl"), data)
                .unwrap();
            assert_eq!(limited.make_dir(Path::new(path)), Err(error), "{path}");
            assert_eq!(one.0.borrow().as_slice(), data, "{path}");
        }
    }

    #[test]
    fn a_hidden_path_through_a_dot_dot_that_finds_no_directory_hides_each_step() {
        let one = One::default();
        let paths = Paths::new(one.clone())
            .within(&["/work".into()], &["/work/moved/../h".into()])
            .unwrap();
        assert_eq!(reached(&paths, "/work/b"), Ok(()));
        Paths::new(one).write(Path::new("/a.hcl"), b"away").unwrap();
        assert_eq!(reached(&paths, "/work/b"), Err(Error::Hidden));
    }

    #[test]
    fn a_hidden_path_through_a_dot_dot_whose_host_path_reaches_nothing_hides_each_step()
    {
        let one = One::default();
        let paths = Paths::new(one.clone())
            .within(&["/work".into()], &["/work/moved/../h".into()])
            .unwrap();
        Paths::new(one.clone())
            .write(Path::new("/a.hcl"), b"moved")
            .unwrap();
        assert_eq!(paths.read(Path::new("/work/b"), 64).map(|_| ()), Ok(()));
        for data in [&b"vanished"[..], b"through", b"circle"] {
            Paths::new(one.clone())
                .write(Path::new("/a.hcl"), data)
                .unwrap();
            let read = paths.read(Path::new("/work/b"), 64).map(|_| ());
            assert_eq!(read, Err(Error::Hidden), "{data:?}");
        }
        Paths::new(one.clone())
            .write(Path::new("/a.hcl"), b"x")
            .unwrap();
        let paths = Paths::new(one.clone())
            .within(&["/work".into()], &["/work/c40/moved/../h".into()])
            .unwrap();
        Paths::new(one).write(Path::new("/a.hcl"), b"away").unwrap();
        let read = paths.read(Path::new("/work/b"), 64).map(|_| ());
        assert_eq!(read, Err(Error::Hidden));
    }

    #[test]
    fn a_dot_dot_whose_host_path_fails_gives_its_error() {
        let paths = Paths::new(One::default());
        paths.write(Path::new("/a.hcl"), b"full").unwrap();
        let resolved = paths.resolve(Path::new("/work/moved/.."));
        assert_eq!(resolved, Err(Error::Io { code: 24 }));
    }

    #[test]
    fn a_dot_dot_takes_the_step_above_that_the_host_gives_with_no_start_again() {
        let one = One::default();
        let calls = Rc::clone(&one.1);
        let paths = Paths::new(one);
        let resolved = paths.resolve(Path::new("/work/firm/../a"));
        assert_eq!(resolved, Ok(PathBuf::from("/a")));
        assert_eq!(calls.get(), 5);
    }

    #[test]
    fn a_dot_dot_to_no_step_follows_the_path_that_the_host_gives_as_a_link() {
        let one = One::default();
        let calls = Rc::clone(&one.1);
        let paths = Paths::new(one);
        let resolved = paths.resolve(Path::new("/work/off/../a"));
        assert_eq!(resolved, Ok(PathBuf::from("/usr/a")));
        assert_eq!(calls.get(), 7);
        let resolved = paths.resolve(Path::new("/work/c39/off/../a"));
        assert_eq!(resolved, Ok(PathBuf::from("/usr/a")));
        let resolved = paths.resolve(Path::new("/work/c40/off/../a"));
        assert_eq!(resolved, Err(Error::Io { code: 40 }));
    }

    #[test]
    fn a_dot_dot_to_a_step_far_above_checks_the_step_that_it_leaves() {
        let paths = Paths::new(One::default())
            .within(&["/in".into()], &[])
            .unwrap();
        let read = paths.read(Path::new("/out/firm/../in/a"), 1);
        assert_eq!(read, Err(Error::Outside));
        let read = paths.read(Path::new("/in/firm/../in/a.hcl"), 1);
        assert_eq!(read, Ok(Found::File(Vec::new())));
    }

    #[test]
    fn a_dot_dot_to_a_step_far_above_leaves_no_step_between() {
        let paths = Paths::new(One::default())
            .within(&["/in".into()], &[])
            .unwrap();
        let read = paths.read(Path::new("/volume/in/firm/../in/a.hcl"), 1);
        assert_eq!(read, Ok(Found::File(Vec::new())));
    }

    #[test]
    fn a_directory_is_not_made_past_a_step_with_an_error() {
        let free = Paths::new(One::default());
        free.write(Path::new("/a.hcl"), b"old").unwrap();
        let paths = free.within(&["/".into()], &[]).unwrap();
        let made = paths.make_dir(Path::new("/work/io/x"));
        assert_eq!(made, Err(Error::Io { code: 5 }));
        assert_eq!(
            free.read(Path::new("/a.hcl"), 3),
            Ok(Found::File(b"old".to_vec()))
        );
    }

    #[test]
    fn a_path_outside_reaches_no_file() {
        let free = Paths::new(One::default());
        free.write(Path::new("/out/a.hcl"), b"old").unwrap();
        let paths = free.within(&["/work".into()], &[]).unwrap();
        assert_eq!(
            paths.write(Path::new("/out/a.hcl"), b"new"),
            Err(Error::Outside)
        );
        assert_eq!(paths.make_dir(Path::new("/out/a.hcl")), Err(Error::Outside));
        assert_eq!(
            free.read(Path::new("/out/a.hcl"), 3),
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
            let not_directory = Err(Error::Io { code: 20 });
            let got = (
                paths.write(Path::new(path), b"y"),
                paths.read(Path::new(path), 1).map(|_| ()),
            );
            assert_eq!(got, (not_directory, not_directory), "{path}");
        }
    }

    #[test]
    fn a_walk_follows_40_links_and_past_them_gives_the_error_of_the_host() {
        let paths = Paths::new(One::default());
        let resolved = paths.resolve(Path::new("/work/c40"));
        assert_eq!(resolved, Ok(PathBuf::from("/work/c0")));
        let looped = Err(Error::Io { code: 40 });
        assert_eq!(paths.resolve(Path::new("/work/c41")), looped);
        paths.write(Path::new("/work/a.hcl"), b"loop").unwrap();
        assert_eq!(paths.resolve(Path::new("/work/loop")), looped);
    }

    #[test]
    fn make_dir_gives_replaced_for_a_loop_at_the_path_of_a_folder() {
        let paths = Paths::new(One::default());
        let within = paths.within(&["/work/loop".into()], &[]).unwrap();
        paths.write(Path::new("/work/a.hcl"), b"loop").unwrap();
        let made = within.make_dir(Path::new("/work/loop"));
        assert_eq!(made, Err(Error::Replaced));
    }

    /// A hidden path that the host refuses at a call (`Io` 13) hides each step that
    /// passes the folders, and the root passes on the way to each. One past a loop or
    /// one that is gone hides nothing.
    #[test]
    fn a_hidden_path_that_the_host_refuses_hides_each_step_in_the_folders() {
        let hidden = (Err(Error::Hidden), Err(Error::Hidden));
        let open = (Ok(()), Err(Error::Outside));
        for (before, after, hidden, (read, out)) in [
            ("x", "gone", "/work/Gone", hidden),
            ("x", "gone", "/work/Gone/k", hidden),
            ("x", "loop", "/work/loop", open),
            ("x", "file", "/work/f/x", open),
            ("x", "file", "/work/f/.", open),
            ("x", "file", "/work/d/f/..", open),
            ("dir", "x", "/none/a/x", open),
        ] {
            let free = Paths::new(One::default());
            free.write(Path::new("/a.hcl"), before.as_bytes()).unwrap();
            let paths = free.within(&["/work".into()], &[hidden.into()]).unwrap();
            free.write(Path::new("/a.hcl"), after.as_bytes()).unwrap();
            let got = [
                paths.read(Path::new("/work/x"), 1).map(|_| ()),
                paths.read(Path::new("/out/x"), 1).map(|_| ()),
            ];
            assert_eq!(got, [read, out], "{hidden}");
        }
    }

    #[test]
    fn make_dir_reaches_the_host_when_the_host_refuses_the_last_name() {
        let paths = Paths::new(One::default())
            .within(&["/work".into()], &[])
            .unwrap();
        assert_eq!(paths.make_dir(Path::new("/work/io")), Ok(()));
        let file = Ok(Found::File(b"dir".to_vec()));
        assert_eq!(paths.read(Path::new("/work/a.hcl"), 3), file);
    }
}
