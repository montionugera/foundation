//! The reads of secret files on one shard.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use env::files::{self, Absolute, Files, Mode};

use crate::Value;
use crate::store::Error;

/// The most bytes that a secret file holds.
const LEN_MAX: u64 = 1 << 16;

/// The reads of secret files on one shard: one read at a time, in the order they
/// come, into a pool that writes zero over its bytes at the drop. Clones share the
/// turn.
#[derive(Clone)]
pub struct Reads(Rc<Inner>);

struct Inner {
    files: Files,
    /// Each read that waits or reads, by its key, with the waker of a read that
    /// waits. The first holds the turn.
    reads: RefCell<BTreeMap<u64, Option<Waker>>>,
    /// The key of the next read.
    next: Cell<u64>,
}

impl Reads {
    /// Reads secret files through `files`.
    #[must_use]
    pub fn new(files: Files) -> Self {
        Self(Rc::new(Inner {
            files,
            reads: RefCell::default(),
            next: Cell::new(0),
        }))
    }

    /// Reads the value in the file at `place`, with one trailing `\n` or `\r\n` cut.
    ///
    /// # Errors
    ///
    /// [`Error::Missing`] when the file is not there or the value is empty.
    /// [`Error::Unavailable`] when the file is more than 64 KiB, or a file call fails.
    pub(crate) async fn read(&self, place: Place<'_>) -> Result<Value, Error> {
        let mut turn = self.turn();
        (&mut turn).await;
        let files = &self.0.files;
        let (opened, path) = match place {
            Place::Data(path) => {
                (files.open(path, Mode::Read).await, files.shown(path))
            }
            Place::Host(path) => {
                (files.open_absolute(path).await, path.as_path().into())
            }
        };
        let file = match opened {
            Ok(file) => file,
            Err(files::Error::NotFound { .. }) => {
                return Err(Error::Missing {
                    detail: format!(
                        "the file {} is not there. Write the value to it",
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
        let config = pool(len);
        let memory = block::Erased::new(config.reservation());
        let pool = block::Pool::new(config, memory);
        let into = pool.alloc(len).expect("invariant: the pool holds the file");
        let read = file.read_at(0, into).await;
        file.close().await;
        let read = read.map_err(|error| unavailable(&error))?;
        let value = cut(&read).to_vec();
        if value.is_empty() {
            return Err(Error::Missing {
                detail: format!(
                    "the file {} is empty. Write the value to it",
                    path.display()
                ),
            });
        }
        Ok(Value::new(value))
    }

    /// Whether no read waits or reads.
    #[cfg(test)]
    pub(crate) fn idle(&self) -> bool {
        self.0.reads.borrow().is_empty()
    }

    /// Puts a read last in the order. The read holds the turn when the result is
    /// ready, until the result drops.
    fn turn(&self) -> Turn<'_> {
        let key = self.0.next.get();
        self.0.next.set(key + 1);
        self.0.reads.borrow_mut().insert(key, None);
        Turn {
            inner: &self.0,
            key,
        }
    }
}

/// Where a secret file is.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Place<'a> {
    /// A path relative to the data directory.
    Data(&'a Path),
    /// A path on the host.
    Host(&'a Absolute),
}

impl fmt::Debug for Reads {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Reads").finish_non_exhaustive()
    }
}

/// A read in the order of [`Reads`]. It is ready when the read holds the turn, and
/// its drop gives the turn to the next read.
struct Turn<'a> {
    inner: &'a Inner,
    key: u64,
}

impl Future for Turn<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut reads = self.inner.reads.borrow_mut();
        if reads.first_key_value().map(|(&key, _)| key) == Some(self.key) {
            return Poll::Ready(());
        }
        let replaced = reads.insert(self.key, Some(cx.waker().clone()));
        // The last drop of a waker can drop a task that uses the reads.
        drop(reads);
        drop(replaced);
        Poll::Pending
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut reads = self.inner.reads.borrow_mut();
        let removed = reads.remove(&self.key);
        // The read that holds the turn keeps no waker, so this wakes a read only
        // when the dropped one held the turn.
        let next = reads
            .first_entry()
            .and_then(|mut next| next.get_mut().take());
        // As in `poll`, each waker drops or wakes after the borrow.
        drop(reads);
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
