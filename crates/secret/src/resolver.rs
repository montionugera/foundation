//! The value of each source of a secret.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

use env::files::Files;

use crate::reads::{LEN_MAX, Place, Reads};
use crate::store::File;
use crate::{Error, Source, Value};

/// Gives the value of each source on one shard. Make one for each shard, and clone it
/// for each user on that shard: the clones read one file at a time, which bounds their
/// heap.
#[derive(Clone)]
pub struct Resolver {
    vars: Arc<BTreeMap<OsString, OsString>>,
    reads: Reads,
    store: File,
}

impl Resolver {
    /// Makes a resolver that reads `env()` from `vars`, `file()` through `files`, and
    /// `secret()` from the directory `dir` of `files`.
    ///
    /// # Panics
    ///
    /// When `dir` is absolute or has a `..` segment.
    #[must_use]
    pub fn new(
        vars: Arc<BTreeMap<OsString, OsString>>,
        files: Files,
        dir: &Path,
    ) -> Self {
        let reads = Reads::new(files);
        let store = File::new(reads.clone(), dir);
        Self { vars, reads, store }
    }

    /// Reads the value of `source`.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when the variable is not set or is empty, or the file is
    /// not there or is empty, with the fix. [`Error::Unavailable`] when the value is
    /// more than 64 KiB, a file call fails, or, for `secret()`, other users can read
    /// its file, with the fix. Each other error of the store for `secret()`.
    pub async fn get(&self, source: &Source) -> Result<Value, Error> {
        match source {
            Source::Env(name) => {
                let missing = |state| Error::Missing {
                    detail: format!(
                        "the environment variable {name} of the node is {state}. \
                         Start the node with it set to the value"
                    ),
                };
                match self.vars.get(OsStr::new(name)) {
                    None => Err(missing("not set")),
                    Some(value) if value.is_empty() => Err(missing("empty")),
                    Some(value) if value.len() as u64 > LEN_MAX => {
                        Err(Error::Unavailable {
                            detail: format!(
                                "the environment variable {name} of the node is {} \
                                 bytes, more than {LEN_MAX}. Start the node with it \
                                 set to at most {LEN_MAX} bytes",
                                value.len()
                            ),
                        })
                    }
                    Some(value) => Ok(Value::new(value.as_encoded_bytes().to_vec())),
                }
            }
            Source::File(path) => self.reads.read(Place::Host(path)).await,
            Source::Secret(name) => self.store.get(name).await,
        }
    }
}

impl fmt::Debug for Resolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Resolver")
            .field("store", &self.store)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
