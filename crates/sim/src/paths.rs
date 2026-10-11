//! The files of each node's host by path, apart from its disk.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use env::paths::{Error, Found};

/// `ENOTDIR` and `EISDIR` on Linux.
const NOT_DIRECTORY: i32 = 20;
const DIRECTORY: i32 = 21;

/// The files of the hosts of a run. A directory is there while a file is under it.
#[derive(Default)]
pub(crate) struct Paths {
    /// The bytes of each file, by node and by path from `/`.
    files: BTreeMap<(usize, PathBuf), Vec<u8>>,
    /// The codes of the faults that wait on each path, by node, in turn.
    faults: BTreeMap<(usize, PathBuf), VecDeque<i32>>,
}

impl Paths {
    pub(crate) fn read(
        &mut self,
        node: usize,
        path: &Path,
        most: usize,
    ) -> Result<Found, Error> {
        let at = self.check(node, path)?;
        if let Some(bytes) = self.files.get(&(node, at.clone())) {
            if bytes.len() > most {
                return Ok(Found::Large);
            }
            return Ok(Found::File(bytes.clone()));
        }
        let names: BTreeSet<_> = self
            .under(node, &at)
            .filter_map(|file| file.strip_prefix(&at).ok()?.components().next())
            .map(Component::as_os_str)
            .collect();
        if names.is_empty() && at != Path::new("/") {
            return Err(Error::NotFound);
        }
        let entries = names.into_iter().map(|name| path.join(name)).collect();
        Ok(Found::Directory(entries))
    }

    /// Writes the file, and so makes each directory above it.
    pub(crate) fn write(
        &mut self,
        node: usize,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), Error> {
        let at = self.check(node, path)?;
        if at == Path::new("/") || self.under(node, &at).next().is_some() {
            return Err(Error::Io { code: DIRECTORY });
        }
        self.files.insert((node, at), bytes.to_vec());
        Ok(())
    }

    pub(crate) fn fail(&mut self, node: usize, path: &Path, code: i32) {
        let key = (node, absolute(path));
        self.faults.entry(key).or_default().push_back(code);
    }

    /// The path of a call from `/`, after the fault that waits on it and a file on
    /// the way to it.
    fn check(&mut self, node: usize, path: &Path) -> Result<PathBuf, Error> {
        let at = absolute(path);
        let fault = self.faults.get_mut(&(node, at.clone()));
        if let Some(code) = fault.and_then(VecDeque::pop_front) {
            return Err(Error::Io { code });
        }
        let mut through = at.ancestors().skip(1);
        if through.any(|above| self.files.contains_key(&(node, above.into()))) {
            return Err(Error::Io {
                code: NOT_DIRECTORY,
            });
        }
        Ok(at)
    }

    /// The path of each file strictly under `dir`.
    fn under<'a>(
        &'a self,
        node: usize,
        dir: &'a Path,
    ) -> impl Iterator<Item = &'a PathBuf> {
        self.files
            .range((node, dir.to_path_buf())..)
            .map(|((on, file), _)| (on, file))
            .take_while(move |(on, file)| **on == node && file.starts_with(dir))
            .map(|(_, file)| file)
            .filter(move |file| *file != dir)
    }
}

/// `path` from `/`, the working directory of each host, with no `.` or `..`.
fn absolute(path: &Path) -> PathBuf {
    let mut at = PathBuf::from("/");
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
