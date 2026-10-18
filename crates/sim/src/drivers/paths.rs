//! The `env::paths` driver of a simulated node.

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use env::paths::dir::Entry;
use env::paths::{Error, Found, Key};

use super::Node;
use crate::paths;
use crate::state::{Shared, lock};
use crate::{LOOP, NOT_DIRECTORY};

impl env::paths::Driver for Node {
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        lock(&self.shared).paths().read(self.node, path, most)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        lock(&self.shared).paths().write(self.node, path, bytes)
    }

    fn make_dir(&self, path: &Path) -> Result<(), Error> {
        lock(&self.shared).paths().make_dir(self.node, path)
    }

    fn home(&self) -> Option<PathBuf> {
        lock(&self.shared).paths().home(self.node)
    }

    fn root(&self, _: &Path) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        Ok(handle(&self.shared, self.node, 0))
    }

    fn not_directory(&self) -> Error {
        Error::Io {
            code: NOT_DIRECTORY,
        }
    }

    fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
        lock(&self.shared).paths().absolute(self.node, path)
    }

    fn looped(&self) -> Error {
        Error::Io { code: LOOP }
    }
}

/// A directory of a node's host, open: its inode, which keeps it across a move.
struct Dir {
    shared: Shared,
    node: usize,
    inode: u64,
}

/// The directory `inode` of `node`, open.
fn handle(
    shared: &Shared,
    node: usize,
    inode: u64,
) -> Box<dyn env::paths::dir::Driver> {
    Box::new(Dir {
        shared: Arc::clone(shared),
        node,
        inode,
    })
}

impl fmt::Debug for Dir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Dir")
            .field(&self.node)
            .field(&self.inode)
            .finish()
    }
}

impl env::paths::dir::Driver for Dir {
    fn key(&self) -> Key {
        Key::new(0, self.inode)
    }

    fn open(&self, name: &OsStr) -> Result<Entry, Error> {
        let mut state = lock(&self.shared);
        let (inode, entry) = state.paths().open(self.node, self.inode, name)?;
        let key = Key::new(0, inode);
        Ok(match entry {
            paths::Entry::File(_) => Entry::File(key),
            paths::Entry::Link(target) => Entry::Link(key, target.clone()),
            paths::Entry::Directory => {
                Entry::Directory(handle(&self.shared, self.node, inode))
            }
        })
    }

    fn up(&self) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        let up = lock(&self.shared).paths().up(self.node, self.inode);
        Ok(handle(&self.shared, self.node, up))
    }
}
