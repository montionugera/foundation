//! The `env::paths` driver of a simulated node.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use env::paths::dir::{Entry, Opened};
use env::paths::{Error, Key, Kind, Refusal};

use super::Node;
use crate::paths;
use crate::state::{Shared, lock};
use crate::{DIRECTORY, EXISTS, INVALID, LOOP, NOT_DIRECTORY};

impl env::paths::Driver for Node {
    fn home(&self) -> Option<PathBuf> {
        lock(&self.shared).paths().home(self.node)
    }

    fn root(&self, _: &Path) -> Result<Box<dyn env::paths::dir::Driver>, Error> {
        Ok(handle(&self.shared, self.node, 0))
    }

    fn absolute(&self, path: &Path) -> Result<PathBuf, Error> {
        lock(&self.shared).paths().absolute(self.node, path)
    }

    fn error(&self, refusal: Refusal) -> Error {
        let code = match refusal {
            Refusal::NotDirectory => NOT_DIRECTORY,
            Refusal::Directory => DIRECTORY,
            Refusal::Taken => EXISTS,
            Refusal::Looped => LOOP,
            Refusal::Irregular => INVALID,
        };
        Error::Io { code }
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

impl Dir {
    /// Runs the change that waits on the call at `name`, or at this directory for no
    /// name, with no lock held, since it changes the files through the node.
    fn race(&self, name: Option<&OsStr>) {
        let change = lock(&self.shared).paths().race(self.node, self.inode, name);
        if let Some(change) = change {
            change();
        }
    }

    /// The file `inode`, open.
    fn opened(&self, inode: u64) -> Box<dyn env::paths::file::Driver> {
        Box::new(File {
            shared: Arc::clone(&self.shared),
            node: self.node,
            inode,
        })
    }
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

    fn entry(&self, name: &OsStr) -> Result<Entry, Error> {
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
        let up = lock(&self.shared).paths().up(self.node, self.inode)?;
        Ok(handle(&self.shared, self.node, up))
    }

    fn path(&self) -> Result<PathBuf, Error> {
        let path = lock(&self.shared).paths().find(self.inode);
        path.ok_or(Error::NotFound)
    }

    fn names(&self, most: usize) -> Result<Option<Vec<OsString>>, Error> {
        self.race(None);
        lock(&self.shared)
            .paths()
            .names(self.node, self.inode, most)
    }

    fn open(&self, name: &OsStr) -> Result<Opened, Error> {
        self.race(Some(name));
        let (inode, kind) = lock(&self.shared)
            .paths()
            .open_file(self.node, self.inode, name)?;
        Ok(match kind {
            Kind::Directory => {
                Opened::Directory(handle(&self.shared, self.node, inode))
            }
            Kind::File => Opened::File(self.opened(inode)),
        })
    }

    fn file(&self, name: &OsStr) -> Result<Box<dyn env::paths::file::Driver>, Error> {
        self.race(Some(name));
        match lock(&self.shared)
            .paths()
            .open_file(self.node, self.inode, name)?
        {
            (_, Kind::Directory) => Err(Error::Io { code: DIRECTORY }),
            (inode, Kind::File) => Ok(self.opened(inode)),
        }
    }

    fn replace(&self, name: &OsStr, bytes: &[u8]) -> Result<(), Error> {
        self.race(Some(name));
        lock(&self.shared)
            .paths()
            .replace(self.node, self.inode, name, bytes)
    }

    fn make_dir(&self, name: &OsStr) -> Result<(), Error> {
        self.race(Some(name));
        lock(&self.shared)
            .paths()
            .make_dir(self.node, self.inode, name)
    }
}

/// A file of a node's host, open: its inode.
struct File {
    shared: Shared,
    node: usize,
    inode: u64,
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("File")
            .field(&self.node)
            .field(&self.inode)
            .finish()
    }
}

impl env::paths::file::Driver for File {
    fn key(&self) -> Key {
        Key::new(0, self.inode)
    }

    fn read(&self, most: usize) -> Result<Option<Vec<u8>>, Error> {
        let bytes = lock(&self.shared).paths().bytes(self.node, self.inode);
        Ok((bytes.len() <= most).then_some(bytes))
    }

    fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        lock(&self.shared)
            .paths()
            .fill(self.node, self.inode, bytes);
        Ok(())
    }
}
