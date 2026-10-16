//! The `env::paths` driver of a simulated node.

use std::path::{Path, PathBuf};

use env::paths::{Error, Found};

use super::Node;
use crate::state::lock;

impl env::paths::Driver for Node {
    fn read(&self, path: &Path, most: usize) -> Result<Found, Error> {
        lock(&self.shared).paths().read(self.node, path, most)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> Result<(), Error> {
        lock(&self.shared).paths().write(self.node, path, bytes)
    }

    fn make_dir(&self, path: &Path) -> Result<(), Error> {
        lock(&self.shared).paths().create_dir(self.node, path)
    }

    fn resolve(&self, path: &Path) -> Result<PathBuf, Error> {
        lock(&self.shared).paths().resolve(self.node, path)
    }

    fn home(&self) -> Option<PathBuf> {
        lock(&self.shared).paths().home(self.node)
    }
}
