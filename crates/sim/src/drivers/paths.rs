//! The `env::paths` driver of a simulated node.

use std::fmt;
use std::path::Path;

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
}

impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Node").field(&self.node).finish()
    }
}
