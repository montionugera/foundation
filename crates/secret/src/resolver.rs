//! The value of each source of a secret.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::sync::Arc;

use crate::store::{self, Store};
use crate::{Reads, Source, Value};

/// Gives the value of each source on one shard.
pub struct Resolver {
    vars: Arc<BTreeMap<OsString, OsString>>,
    reads: Reads,
    store: Box<dyn Store>,
}

impl Resolver {
    /// Makes a resolver that reads `env()` from `vars`, `file()` through `reads`, and
    /// `secret()` from `store`.
    #[must_use]
    pub fn new(
        vars: Arc<BTreeMap<OsString, OsString>>,
        reads: Reads,
        store: Box<dyn Store>,
    ) -> Self {
        Self { vars, reads, store }
    }

    /// Reads the value of `source`.
    ///
    /// # Errors
    ///
    /// [`store::Error::Missing`] when the variable is not set or is empty, or the
    /// file is not there or is empty, with the fix. [`store::Error::Unavailable`]
    /// when the file holds more than 64 KiB or a file call fails. Each error of the
    /// store for `secret()`.
    pub async fn get(&self, source: &Source) -> Result<Value, store::Error> {
        match source {
            Source::Env(name) => {
                let missing = |state| store::Error::Missing {
                    detail: format!(
                        "the environment variable {name} of the node is {state}. \
                         Start the node with it set to the value"
                    ),
                };
                match self.vars.get(OsStr::new(name)) {
                    None => Err(missing("not set")),
                    Some(value) if value.is_empty() => Err(missing("empty")),
                    Some(value) => Ok(Value::new(value.as_encoded_bytes().to_vec())),
                }
            }
            Source::File(path) => self.reads.read(path).await,
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
