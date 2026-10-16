//! The files of each node's host by path, apart from its disk.

use std::collections::btree_map::Entry::{Occupied, Vacant};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use env::paths::{Error, Found, names_directory};

use crate::{DIRECTORY, EXISTS, LOOP, NOT_DIRECTORY};

/// The most symbolic links that one path goes through, as on Linux.
const LINKS: usize = 40;

/// The files of the hosts of a run.
#[derive(Default)]
pub(crate) struct Paths {
    /// Each file, symbolic link, and directory, by node and by path from `/`.
    entries: BTreeMap<(usize, PathBuf), Entry>,
    /// The codes of the faults that wait on each path, by node, in turn.
    faults: BTreeMap<(usize, PathBuf), VecDeque<i32>>,
    /// The working directory of each node that set one, else `/`.
    working: BTreeMap<usize, PathBuf>,
    homes: BTreeMap<usize, PathBuf>,
}

enum Entry {
    File(Vec<u8>),
    /// A symbolic link: the path that it names.
    Link(PathBuf),
    Directory,
}

impl Paths {
    pub(crate) fn read(
        &mut self,
        node: usize,
        path: &Path,
        most: usize,
    ) -> Result<Found, Error> {
        let at = self.check(node, path)?;
        if let Some(Entry::File(bytes)) = self.entries.get(&(node, at.clone())) {
            if bytes.len() > most {
                return Ok(Found::Large);
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
            return Ok(Found::Large);
        }
        Ok(Found::Directory(entries))
    }

    pub(crate) fn write(
        &mut self,
        node: usize,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let at = self.check(node, path)?;
        let directory = self.directory(node, &at);
        if names_directory(path) {
            return Err(if directory {
                Error::Io { code: DIRECTORY }
            } else {
                Error::NotFound
            });
        }
        if directory {
            return Err(Error::Io { code: DIRECTORY });
        }
        self.entries.insert((node, at), Entry::File(bytes.to_vec()));
        Ok(())
    }

    /// Makes a directory at `path` as `os` makes one: only in a directory that is
    /// there, `Ok` when a directory is there, else `EEXIST` when anything is there.
    pub(crate) fn create_dir(&mut self, node: usize, path: &Path) -> Result<(), Error> {
        let at = match self.check(node, path) {
            Err(Error::NotFound) if self.linked(node, path) => {
                return Err(Error::Io { code: EXISTS });
            }
            at => at?,
        };
        if self.directory(node, &at) {
            return Ok(());
        }
        match self.entries.entry((node, at)) {
            Vacant(entry) => {
                entry.insert(Entry::Directory);
                Ok(())
            }
            Occupied(_) => Err(Error::Io { code: EXISTS }),
        }
    }

    /// `path` from the working directory, with each `.`, `..`, and symbolic link
    /// resolved, as `os` resolves it.
    pub(crate) fn resolve(&self, node: usize, path: &Path) -> Result<PathBuf, Error> {
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
                    if let Some(Entry::Link(target)) =
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

    pub(crate) fn home(&self, node: usize) -> Option<PathBuf> {
        let home = self.homes.get(&node)?;
        self.resolve(node, home).ok()
    }

    pub(crate) fn fail(&mut self, node: usize, path: &Path, code: i32) {
        let key = (node, self.absolute(node, path));
        self.faults.entry(key).or_default().push_back(code);
    }

    /// Makes a symbolic link at `path`, which is absolute with no `..`, and so each
    /// directory above it.
    pub(crate) fn link(&mut self, node: usize, path: &Path, target: &Path) {
        let at = self.place(node, path);
        let there = self.entries.contains_key(&(node, at.clone()));
        assert!(!there, "{} is there", path.display());
        self.entries
            .insert((node, at), Entry::Link(target.to_path_buf()));
    }

    /// Makes a directory at `path`, which is absolute with no `..`, and so each
    /// directory above it.
    pub(crate) fn make_dir(&mut self, node: usize, path: &Path) {
        let at = self.place(node, path);
        let entry = self.entries.entry((node, at));
        let there =
            matches!(&entry, Occupied(on) if !matches!(on.get(), Entry::Directory));
        assert!(!there, "{} is there", path.display());
        entry.or_insert(Entry::Directory);
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
        self.entries
            .entry((node, at.clone()))
            .or_insert(Entry::Directory);
        at.join(name)
    }

    /// Whether a symbolic link is at the last name of `path`.
    fn linked(&self, node: usize, path: &Path) -> bool {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let dir = if dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            dir
        };
        self.resolve(node, dir).is_ok_and(|dir| {
            let entry = self.entries.get(&(node, dir.join(name)));
            matches!(entry, Some(Entry::Link(_)))
        })
    }

    /// The resolved path of a call, after the fault that waits on its path.
    fn check(&mut self, node: usize, path: &Path) -> Result<PathBuf, Error> {
        if path.as_os_str().is_empty() {
            return Err(Error::NotFound);
        }
        let fault = self.faults.get_mut(&(node, self.absolute(node, path)));
        if let Some(code) = fault.and_then(VecDeque::pop_front) {
            return Err(Error::Io { code });
        }
        self.resolve(node, path)
    }

    fn working(&self, node: usize) -> PathBuf {
        let working = self.working.get(&node);
        working.map_or_else(|| PathBuf::from("/"), PathBuf::clone)
    }

    fn directory(&self, node: usize, path: &Path) -> bool {
        let entry = self.entries.get(&(node, path.to_path_buf()));
        path == Path::new("/") || matches!(entry, Some(Entry::Directory))
    }

    fn file(&self, node: usize, path: &Path) -> bool {
        let entry = self.entries.get(&(node, path.to_path_buf()));
        matches!(entry, Some(Entry::File(_)))
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
    fn absolute(&self, node: usize, path: &Path) -> PathBuf {
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
