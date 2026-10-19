//! The files of each node's host by path, apart from its disk.

use std::collections::btree_map::Entry::Occupied;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use env::paths::file::Access;
use env::paths::{Error, Found, Kind, LINKS, names_directory};

use crate::{DIRECTORY, EXISTS, INVALID, LOOP, NOT_DIRECTORY, PATH_MAX, TOO_LONG};

/// A change that a test makes to the files during a call.
pub(crate) type Change = Box<dyn FnOnce() + Send>;

/// The files of the hosts of a run.
#[derive(Default)]
pub(crate) struct Paths {
    /// Each file, symbolic link, and directory, by node and by path from `/`, with
    /// its inode.
    entries: BTreeMap<(usize, PathBuf), (u64, Entry)>,
    /// The faults that wait, in the order set: the node, the path as given, and the
    /// code. A relative path takes the working directory at the call.
    faults: Vec<(usize, PathBuf, i32)>,
    /// The changes that wait, as `faults` do.
    races: Vec<(usize, PathBuf, Change)>,
    /// The working directory of each node that set one, else `/`.
    working: BTreeMap<usize, PathBuf>,
    homes: BTreeMap<usize, PathBuf>,
    /// The last inode made. `/` is inode 0, and the device of each key is 0.
    made: u64,
}

pub(crate) enum Entry {
    File(Vec<u8>),
    /// A symbolic link: the path that it names.
    Link(PathBuf),
    Directory,
}

const ROOT: &Entry = &Entry::Directory;

impl Paths {
    pub(crate) fn read(
        &mut self,
        node: usize,
        path: &Path,
        most: usize,
    ) -> Result<Found, Error> {
        bound(path)?;
        let at = self.resolve(node, path)?;
        self.fault(node, &at)?;
        if let Some((_, Entry::File(bytes))) = self.entries.get(&(node, at.clone())) {
            if bytes.len() > most {
                return Ok(Found::Large(Kind::File));
            }
            return Ok(Found::File(bytes.clone()));
        }
        if !self.directory(node, &at) {
            return Err(Error::NotFound);
        }
        let names: BTreeSet<_> = self
            .under(node, &at)
            .filter_map(|entry| entry.strip_prefix(&at).ok()?.components().next())
            .map(Component::as_os_str)
            .collect();
        let entries: Vec<_> = names.into_iter().map(|name| path.join(name)).collect();
        let size = entries.iter().map(|entry| entry.as_os_str().len());
        if size.fold(0_usize, usize::saturating_add) > most {
            return Ok(Found::Large(Kind::Directory));
        }
        Ok(Found::Directory(entries))
    }

    /// The name of each entry of the directory `dir`, or `None` when they hold more
    /// than `most` bytes in all.
    pub(crate) fn names(
        &mut self,
        node: usize,
        dir: u64,
        most: usize,
    ) -> Result<Option<Vec<OsString>>, Error> {
        let at = self.call(node, dir, None)?;
        let names: BTreeSet<_> = self
            .under(node, &at)
            .filter_map(|entry| entry.strip_prefix(&at).ok()?.components().next())
            .map(|name| name.as_os_str().to_owned())
            .collect();
        let size = names.iter().map(|name| name.len());
        let large = size.fold(0_usize, usize::saturating_add) > most;
        Ok((!large).then(|| names.into_iter().collect()))
    }

    /// The inode of the file at `name` in the directory `dir`, open for `access`, as
    /// `os` opens one: `ELOOP` for a link, and `EISDIR` for a directory open to write.
    pub(crate) fn open_file(
        &mut self,
        node: usize,
        dir: u64,
        name: &OsStr,
        access: Access,
    ) -> Result<u64, Error> {
        let at = self.call(node, dir, Some(name))?;
        match self.at(node, &at) {
            None => Err(Error::NotFound),
            Some((_, Entry::Link(_))) => Err(Error::Io { code: LOOP }),
            Some((_, Entry::Directory)) if access == Access::Write => {
                Err(Error::Io { code: DIRECTORY })
            }
            Some((inode, _)) => Ok(inode),
        }
    }

    /// The bytes of the open file `inode`, or `EINVAL` for a directory.
    pub(crate) fn bytes(&self, node: usize, inode: u64) -> Result<Vec<u8>, Error> {
        match self.opened(node, inode) {
            Entry::File(bytes) => Ok(bytes.clone()),
            _ => Err(Error::Io { code: INVALID }),
        }
    }

    /// Writes `bytes` in place in the open file `inode`.
    pub(crate) fn fill(&mut self, node: usize, inode: u64, bytes: &[u8]) {
        let found = self
            .entries
            .iter_mut()
            .find(|((on, _), (at, _))| *on == node && *at == inode);
        if let Some((_, (_, Entry::File(old)))) = found {
            *old = bytes.to_vec();
        }
    }

    /// Writes `bytes` as a new file at `name` in the directory `dir`, as the rename of
    /// a write of `os` makes one.
    pub(crate) fn replace(
        &mut self,
        node: usize,
        dir: u64,
        name: &OsStr,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let at = self.call(node, dir, Some(name))?;
        if self.directory(node, &at) {
            return Err(Error::Io { code: DIRECTORY });
        }
        let inode = self.inode();
        self.entries
            .insert((node, at), (inode, Entry::File(bytes.to_vec())));
        Ok(())
    }

    /// Makes the directory `name` in the directory `dir`: `Ok` when a directory is
    /// there, and `EEXIST` when anything else is, with no link followed.
    pub(crate) fn make_dir(
        &mut self,
        node: usize,
        dir: u64,
        name: &OsStr,
    ) -> Result<(), Error> {
        let at = self.call(node, dir, Some(name))?;
        match self.at(node, &at) {
            None => {
                let inode = self.inode();
                self.entries.insert((node, at), (inode, Entry::Directory));
                Ok(())
            }
            Some((_, Entry::Directory)) => Ok(()),
            Some(_) => Err(Error::Io { code: EXISTS }),
        }
    }

    /// The change that waits on the call at `name` in the directory `dir`, or at `dir`
    /// for no name. The caller runs it with no lock held.
    pub(crate) fn race(
        &mut self,
        node: usize,
        dir: u64,
        name: Option<&OsStr>,
    ) -> Option<Change> {
        let mut at = self.find(dir);
        at.extend(name);
        let race = self.races.iter().position(|(on, given, _)| {
            *on == node && self.lexical(node, given) == at
        })?;
        Some(self.races.remove(race).2)
    }

    /// `path` from the working directory, with each `.`, `..`, and symbolic link
    /// resolved, as the host resolves it for an open.
    fn resolve(&self, node: usize, path: &Path) -> Result<PathBuf, Error> {
        if path.as_os_str().is_empty() {
            return Err(Error::NotFound);
        }
        let mut at = PathBuf::from("/");
        let mut parts: Vec<_> = path.components().rev().collect();
        let working = self.working(node);
        if path.is_relative() {
            parts.extend(working.components().rev());
        }
        // `components` drops an end in `/` or `.`.
        let (mut links, mut last_linked) = (0, false);
        let mut named_directory = names_directory(path);
        while let Some(part) = parts.pop() {
            if self.file(node, &at) {
                return Err(Error::Io {
                    code: NOT_DIRECTORY,
                });
            }
            if !self.directory(node, &at) {
                return Err(Error::NotFound);
            }
            match part {
                Component::Prefix(_) | Component::RootDir => at = PathBuf::from("/"),
                Component::CurDir => {}
                Component::ParentDir => {
                    at.pop();
                }
                Component::Normal(name) => {
                    at.push(name);
                    if let Some((_, Entry::Link(target))) =
                        self.entries.get(&(node, at.clone()))
                    {
                        links += 1;
                        if links > LINKS {
                            return Err(Error::Io { code: LOOP });
                        }
                        if parts.is_empty() {
                            last_linked = true;
                            named_directory |= names_directory(target);
                        }
                        at.pop();
                        parts.extend(target.components().rev());
                    }
                }
            }
        }
        if named_directory && self.file(node, &at) {
            return Err(Error::Io {
                code: NOT_DIRECTORY,
            });
        }
        if last_linked && !self.file(node, &at) && !self.directory(node, &at) {
            return Err(Error::NotFound);
        }
        Ok(at)
    }

    /// The inode and entry at `name` in the directory `dir`, by its inode, with no
    /// link followed. `NotFound` when nothing is at `name`.
    pub(crate) fn open(
        &self,
        node: usize,
        dir: u64,
        name: &OsStr,
    ) -> Result<(u64, &Entry), Error> {
        self.at(node, &self.find(dir).join(name))
            .ok_or(Error::NotFound)
    }

    /// The inode of the directory above the directory `dir`, and `/` above `/`.
    pub(crate) fn up(&self, node: usize, dir: u64) -> u64 {
        let at = self.find(dir);
        let above = self.at(node, at.parent().unwrap_or(&at));
        let (inode, _) =
            above.expect("invariant: the directory above a directory is there");
        inode
    }

    /// The path of the directory `inode` now.
    pub(crate) fn find(&self, inode: u64) -> PathBuf {
        if inode == 0 {
            return PathBuf::from("/");
        }
        // Each inode of a run is of one entry, and no call removes a directory.
        let found = self.entries.iter().find(|(_, (at, _))| *at == inode);
        let ((_, path), _) = found.unwrap_or_else(|| {
            panic!("invariant: directory inode {inode} has an entry")
        });
        path.clone()
    }

    /// `path` on the resolved working directory, with each `..` kept, as
    /// `std::path::absolute` gives it. `NotFound` when the working directory is not
    /// there, or the error of [`bound`].
    pub(crate) fn absolute(&self, node: usize, path: &Path) -> Result<PathBuf, Error> {
        bound(path)?;
        let mut at = PathBuf::new();
        if !path.has_root() {
            at = self.resolve(node, &self.working(node))?;
            if !self.directory(node, &at) {
                return Err(Error::NotFound);
            }
        }
        at.extend(path.components().filter(|part| *part != Component::CurDir));
        Ok(at)
    }

    /// A new inode.
    fn inode(&mut self) -> u64 {
        self.made += 1;
        self.made
    }

    pub(crate) fn home(&self, node: usize) -> Option<PathBuf> {
        self.homes.get(&node).cloned()
    }

    pub(crate) fn fail(&mut self, node: usize, path: &Path, code: i32) {
        self.faults.push((node, path.to_path_buf(), code));
    }

    pub(crate) fn add_race(&mut self, node: usize, path: &Path, change: Change) {
        self.races.push((node, path.to_path_buf(), change));
    }

    /// Makes a symbolic link at `path`, which is absolute with no `..`, and so each
    /// directory above it.
    pub(crate) fn link(&mut self, node: usize, path: &Path, target: &Path) {
        let at = self.place(node, path);
        let there = self.entries.contains_key(&(node, at.clone()));
        assert!(!there, "{} is there", path.display());
        let inode = self.inode();
        self.entries
            .insert((node, at), (inode, Entry::Link(target.to_path_buf())));
    }

    /// Makes a directory at `path`, which is absolute with no `..`, and so each
    /// directory above it.
    pub(crate) fn place_dir(&mut self, node: usize, path: &Path) {
        let at = self.place(node, path);
        let inode = self.inode();
        let entry = self.entries.entry((node, at));
        let there =
            matches!(&entry, Occupied(on) if !matches!(on.get().1, Entry::Directory));
        assert!(!there, "{} is there", path.display());
        entry.or_insert((inode, Entry::Directory));
    }

    /// Moves the entry at `from` to `to`, both absolute with no `..`, with each entry
    /// under it and their inodes. A link at the last name of either is not followed.
    /// The working directory stays on the directory that it resolves to, moved with
    /// `from` when it is at or under it, as a host keeps a process on the directory
    /// itself. Its path changes only when it no longer resolves there.
    pub(crate) fn move_entry(&mut self, node: usize, from: &Path, to: &Path) {
        let from = self.site(node, from);
        let to = self.site(node, to);
        assert!(
            self.entries.contains_key(&(node, from.clone())),
            "nothing is at {}",
            from.display()
        );
        let there = self.entries.contains_key(&(node, to.clone()));
        assert!(!there, "{} is there", to.display());
        assert!(
            !to.starts_with(&from),
            "{} is under {}",
            to.display(),
            from.display()
        );
        let given = self.working.get(&node).cloned();
        let working = given.as_ref().and_then(|dir| self.resolve(node, dir).ok());
        let mut moved: Vec<_> = self.under(node, &from).cloned().collect();
        moved.push(from.clone());
        for path in moved {
            let entry = self.entries.remove(&(node, path.clone()));
            let entry = entry.expect("invariant: each moved path has an entry");
            let path = to.join(path.strip_prefix(&from).expect("under `from`"));
            self.entries.insert((node, path), entry);
        }
        if let (Some(given), Some(dir)) = (given, working) {
            let dir = match dir.strip_prefix(&from) {
                Ok(under) => to.join(under),
                Err(_) => dir,
            };
            // The text stays while it still resolves there, so a fault keeps its
            // spelling across a move elsewhere.
            if self.resolve(node, &given).ok().as_ref() != Some(&dir) {
                self.working.insert(node, dir);
            }
        }
    }

    pub(crate) fn set_working(&mut self, node: usize, dir: &Path) {
        self.working.insert(node, dir.to_path_buf());
    }

    pub(crate) fn set_home(&mut self, node: usize, home: &Path) {
        self.homes.insert(node, home.to_path_buf());
    }

    /// `path` with its directory resolved, made with each directory above it that is
    /// not there.
    ///
    /// # Panics
    ///
    /// When `path` is `/`, or its directory goes through a file, a link to nothing, or
    /// more than 40 links.
    fn place(&mut self, node: usize, path: &Path) -> PathBuf {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            panic!("{} is there", path.display());
        };
        let at = match self.resolve(node, dir) {
            Ok(at) if self.file(node, &at) => Err(Error::Io {
                code: NOT_DIRECTORY,
            }),
            Ok(at) => Ok(at),
            Err(Error::NotFound) => {
                let at = self.place(node, dir);
                if self.entries.contains_key(&(node, at.clone())) {
                    Err(Error::NotFound)
                } else {
                    Ok(at)
                }
            }
            Err(e) => Err(e),
        };
        let at = at.unwrap_or_else(|e| panic!("{} gave {e:?}", path.display()));
        if self.at(node, &at).is_none() {
            let inode = self.inode();
            self.entries
                .insert((node, at.clone()), (inode, Entry::Directory));
        }
        at.join(name)
    }

    /// `path` with its directory resolved, which is there.
    ///
    /// # Panics
    ///
    /// When `path` is `/`, or its directory does not resolve or is not a directory.
    fn site(&self, node: usize, path: &Path) -> PathBuf {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            panic!("{} has no directory", path.display());
        };
        let at = self.resolve(node, dir);
        let at = at.unwrap_or_else(|e| panic!("{} gave {e:?}", dir.display()));
        assert!(
            self.directory(node, &at),
            "{} is not a directory",
            dir.display()
        );
        at.join(name)
    }

    /// The path of the call at `name` in the directory `dir`, or at `dir` for no
    /// name, after the fault that waits on it.
    fn call(
        &mut self,
        node: usize,
        dir: u64,
        name: Option<&OsStr>,
    ) -> Result<PathBuf, Error> {
        let mut at = self.find(dir);
        at.extend(name);
        self.fault(node, &at)?;
        Ok(at)
    }

    /// The fault that waits on `at`, a path from `/` with no link in it.
    fn fault(&mut self, node: usize, at: &Path) -> Result<(), Error> {
        let fault = self
            .faults
            .iter()
            .position(|(on, given, _)| *on == node && self.lexical(node, given) == at);
        match fault.map(|fault| self.faults.remove(fault)) {
            Some((.., code)) => Err(Error::Io { code }),
            None => Ok(()),
        }
    }

    fn working(&self, node: usize) -> PathBuf {
        let working = self.working.get(&node);
        working.map_or_else(|| PathBuf::from("/"), PathBuf::clone)
    }

    /// The inode and entry at `path`, a path from `/` with no link in it. `/` is
    /// inode 0, with no entry in `entries`.
    fn at(&self, node: usize, path: &Path) -> Option<(u64, &Entry)> {
        if path == Path::new("/") {
            return Some((0, ROOT));
        }
        let found = self.entries.get(&(node, path.to_path_buf()));
        found.map(|(inode, entry)| (*inode, entry))
    }

    /// The entry of the open file `inode`.
    ///
    /// # Panics
    ///
    /// When no entry has `inode`: no change runs between an open and its call.
    fn opened(&self, node: usize, inode: u64) -> &Entry {
        let found = self
            .entries
            .iter()
            .find(|((on, _), (at, _))| *on == node && *at == inode);
        let (_, (_, entry)) = found.expect("invariant: an open file has an entry");
        entry
    }

    fn directory(&self, node: usize, path: &Path) -> bool {
        matches!(self.at(node, path), Some((_, Entry::Directory)))
    }

    fn file(&self, node: usize, path: &Path) -> bool {
        let entry = self.entries.get(&(node, path.to_path_buf()));
        matches!(entry, Some((_, Entry::File(_))))
    }

    /// The path of each entry strictly under `dir`.
    fn under<'a>(
        &'a self,
        node: usize,
        dir: &'a Path,
    ) -> impl Iterator<Item = &'a PathBuf> {
        self.entries
            .range((node, dir.to_path_buf())..)
            .map(|((on, entry), _)| (on, entry))
            .take_while(move |(on, entry)| **on == node && entry.starts_with(dir))
            .map(|(_, entry)| entry)
            .filter(move |entry| *entry != dir)
    }

    /// `path` from the working directory, with `.` and `..` taken as text.
    fn lexical(&self, node: usize, path: &Path) -> PathBuf {
        let mut at = self.working(node);
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => at = PathBuf::from("/"),
                Component::CurDir => {}
                Component::ParentDir => {
                    at.pop();
                }
                Component::Normal(name) => at.push(name),
            }
        }
        at
    }
}

/// `ENAMETOOLONG` for a path that [`PATH_MAX`] does not hold.
fn bound(path: &Path) -> Result<(), Error> {
    if path.as_os_str().len() >= PATH_MAX {
        return Err(Error::Io { code: TOO_LONG });
    }
    Ok(())
}
