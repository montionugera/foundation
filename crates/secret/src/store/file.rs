use std::fmt;
use std::path::{Component, Path, PathBuf};

use types::name::Name;

use crate::reads::{Place, Reads};
use crate::{Error, Value};

/// A store that holds each secret in a file of its own under one directory of the
/// data directory: the value of `plant.influx_token` is the file
/// `<dir>/plant.influx_token`, with one trailing `\n` or `\r\n` cut. It reads the
/// file at each `get`, so a file added or changed takes effect at the next `get`.
/// It holds no name with an ASCII capital, so no two names share a file on a file
/// system that ignores case. It refuses a file that other users can read, and reads
/// no byte of it. Its gets take turns with each other read of its [`Reads`].
#[derive(Clone)]
pub(crate) struct File {
    reads: Reads,
    dir: PathBuf,
}

impl File {
    /// Makes a store of the files in `dir`, a path relative to the data directory of
    /// `reads`. `dir` need not be there yet.
    ///
    /// # Panics
    ///
    /// When `dir` is absolute or has a `..` segment.
    #[must_use]
    pub(crate) fn new(reads: Reads, dir: &Path) -> Self {
        assert!(
            dir.is_relative() && !dir.components().any(|c| c == Component::ParentDir),
            "a secret directory is relative with no `..`: {}",
            dir.display()
        );
        Self {
            reads,
            dir: dir.to_path_buf(),
        }
    }

    /// Reads the value of `name` from its file.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when `dir` or the file is not there, or the value is empty,
    /// with the path to write it to, as
    /// [`env::files::Files::shown`] gives it.
    /// [`Error::BadName`] when `name` has an ASCII capital. [`Error::Unavailable`]
    /// when other users can read the file, the file is more than 64 KiB, or a file
    /// call fails, with the fix.
    pub(crate) async fn get(&self, name: &Name) -> Result<Value, Error> {
        if name.as_str().bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(Error::BadName {
                detail: "Rename the secret in lowercase letters, since the file store \
                         holds no name with a capital letter"
                    .into(),
            });
        }
        (self.reads)
            .read(Place::Data(&self.dir.join(name.as_str())))
            .await
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
