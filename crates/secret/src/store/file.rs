use std::fmt;
use std::path::{Component, Path, PathBuf};

use env::files::{self, Files, Mode};
use types::name::Name;
use zeroize::Zeroize;

use super::{Error, Request, Store};
use crate::Value;

/// The most bytes the file of a value holds.
const LEN_MAX: u64 = 1 << 16;

/// A store that holds each secret in a file of its own under one directory of the
/// data directory: the value of `plant.influx_token` is the file
/// `<dir>/plant.influx_token`, with one trailing `\n` or `\r\n` cut. It reads the
/// file at each `get`, so a file added or changed takes effect at the next `get`.
/// It holds no name with an ASCII capital, so no two names share a file on a file
/// system that ignores case. It does not check the owner or mode of the file.
pub struct File {
    files: Files,
    dir: PathBuf,
}

impl File {
    /// Makes a store of the files in `dir`, a path relative to `files`. `dir` need
    /// not be there yet.
    ///
    /// # Panics
    ///
    /// When `dir` is absolute or has a `..` segment.
    #[must_use]
    pub fn new(files: Files, dir: &Path) -> Self {
        assert!(
            dir.is_relative() && !dir.components().any(|c| c == Component::ParentDir),
            "a secret directory is relative with no `..`: {}",
            dir.display()
        );
        Self {
            files,
            dir: dir.to_path_buf(),
        }
    }

    async fn read(&self, name: &Name) -> Result<Value, Error> {
        if name.as_str().bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(Error::Missing {
                detail: "the file store holds no name with a capital letter. Rename \
                         the secret in lowercase letters"
                    .into(),
            });
        }
        let path = self.dir.join(name.as_str());
        let file = match self.files.open(&path, Mode::Read).await {
            Ok(file) => file,
            Err(files::Error::NotFound { .. }) => {
                return Err(Error::Missing {
                    detail: format!(
                        "write it to the file {} of the data directory",
                        path.display()
                    ),
                });
            }
            Err(error) => return Err(unavailable(&error)),
        };
        if file.len() > LEN_MAX {
            return Err(Error::Unavailable {
                detail: format!(
                    "the file {} is {} bytes, more than {LEN_MAX}",
                    path.display(),
                    file.len()
                ),
            });
        }
        let len = usize::try_from(file.len()).expect("invariant: at most `LEN_MAX`");
        let pool = block::Pool::heap(pool(len));
        let into = pool.alloc(len).expect("invariant: the pool holds the file");
        let read = file.read_at(0, into).await;
        file.close().await;
        let mut read = read.map_err(|error| unavailable(&error))?;
        let value = cut(&read).to_vec();
        read.zeroize();
        if value.is_empty() {
            return Err(Error::Missing {
                detail: format!(
                    "the file {} of the data directory is empty. Write the value to it",
                    path.display()
                ),
            });
        }
        Ok(Value::new(value))
    }
}

impl Store for File {
    /// Reads the value of `name` from its file.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when `dir` or the file is not there, or the value is empty,
    /// with the path to write it to, and when `name` has an ASCII capital, with the
    /// fix to rename it. [`Error::Unavailable`] when the file is more than 64 KiB, or
    /// a file call fails, with the call's error.
    fn get<'a>(&'a self, name: &'a Name) -> Request<'a> {
        Box::pin(self.read(name))
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// The settings of a pool that holds one block of `len` bytes.
fn pool(len: usize) -> block::Config {
    let budget = u64::try_from(len).expect("invariant: at most `LEN_MAX`") * 2 + 4096;
    block::Config::new(budget).expect("invariant: a budget of at most 132 KiB fits")
}

/// `bytes` with one trailing `\n` or `\r\n` cut.
fn cut(bytes: &[u8]) -> &[u8] {
    match bytes.strip_suffix(b"\n") {
        Some(line) => line.strip_suffix(b"\r").unwrap_or(line),
        None => bytes,
    }
}

fn unavailable(error: &files::Error) -> Error {
    Error::Unavailable {
        detail: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
