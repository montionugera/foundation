use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use env::files::{self, Files, Mode};
use types::name::Name;

use super::{Error, Request, Store};
use crate::Value;

/// The most bytes the file of a value holds.
const LEN_MAX: u64 = 1 << 16;

/// A store that holds each secret in a file of its own under one directory of the
/// data directory: the value of `plant.influx_token` is the file
/// `<dir>/plant.influx_token`, with one trailing `\n` or `\r\n` cut. It reads the
/// file at each `get`, so a file added or changed takes effect at the next `get`.
/// It holds no name with an ASCII capital, so no two names share a file on a file
/// system that ignores case. It does not check the owner or mode of the file, so make
/// the file readable only by the user that runs the node. It runs one `get` at a time,
/// and the others wait in the order they came.
pub struct File {
    files: Files,
    dir: PathBuf,
    /// Each get that waits or reads, by its key, with the waker of a get that waits.
    /// The first holds the turn.
    gets: RefCell<BTreeMap<u64, Option<Waker>>>,
    /// The key of the next get.
    next: Cell<u64>,
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
            gets: RefCell::default(),
            next: Cell::new(0),
        }
    }

    /// Puts a get last in the order. The get holds the turn when the result is
    /// ready, until the result drops.
    fn turn(&self) -> Turn<'_> {
        let key = self.next.get();
        self.next.set(key + 1);
        self.gets.borrow_mut().insert(key, None);
        Turn { store: self, key }
    }

    async fn read(&self, name: &Name) -> Result<Value, Error> {
        if name.as_str().bytes().any(|byte| byte.is_ascii_uppercase()) {
            return Err(Error::BadName {
                detail: "Rename the secret in lowercase letters, since the file store \
                         holds no name with a capital letter"
                    .into(),
            });
        }
        let mut turn = self.turn();
        (&mut turn).await;
        let path = self.dir.join(name.as_str());
        let file = match self.files.open(&path, Mode::Read).await {
            Ok(file) => file,
            Err(files::Error::NotFound { .. }) => {
                return Err(Error::Missing {
                    detail: format!(
                        "write it to the file {}",
                        self.files.shown(&path).display()
                    ),
                });
            }
            Err(error) => return Err(unavailable(&error)),
        };
        if file.len() > LEN_MAX {
            return Err(Error::Unavailable {
                detail: format!(
                    "the file {} is {} bytes, more than {LEN_MAX}",
                    self.files.shown(&path).display(),
                    file.len()
                ),
            });
        }
        let len = usize::try_from(file.len()).expect("invariant: at most `LEN_MAX`");
        let config = pool(len);
        let pool = block::Pool::erased(config);
        let into = pool.alloc(len).expect("invariant: the pool holds the file");
        let read = file.read_at(0, into).await;
        file.close().await;
        let read = read.map_err(|error| unavailable(&error))?;
        let value = cut(&read).to_vec();
        if value.is_empty() {
            return Err(Error::Missing {
                detail: format!(
                    "the file {} is empty. Write the value to it",
                    self.files.shown(&path).display()
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
    /// with the path to write it to, as [`Files::shown`] gives it.
    /// [`Error::BadName`] when `name` has an ASCII capital. [`Error::Unavailable`]
    /// when the file is more than 64 KiB, or a file call fails, with the call's
    /// error.
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

/// A get in the order of [`File`]. It is ready when the get holds the turn, and its
/// drop gives the turn to the next get.
struct Turn<'a> {
    store: &'a File,
    key: u64,
}

impl Future for Turn<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut gets = self.store.gets.borrow_mut();
        if gets.first_key_value().map(|(&key, _)| key) == Some(self.key) {
            return Poll::Ready(());
        }
        let replaced = gets.insert(self.key, Some(cx.waker().clone()));
        // The last drop of a waker can drop a task that uses the store.
        drop(gets);
        drop(replaced);
        Poll::Pending
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut gets = self.store.gets.borrow_mut();
        let removed = gets.remove(&self.key);
        // The get that holds the turn keeps no waker, so this wakes a get only when
        // the dropped one held the turn.
        let next = gets
            .first_entry()
            .and_then(|mut next| next.get_mut().take());
        // As in `poll`, each waker drops or wakes after the borrow.
        drop(gets);
        drop(removed);
        if let Some(next) = next {
            next.wake();
        }
    }
}

/// The settings of a pool that holds one block of `len` bytes. The budget is twice
/// `len` and 4 KiB, since `block` rounds a block up to its size class.
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
