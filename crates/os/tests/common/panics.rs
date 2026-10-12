//! Helpers that the `it` and `aborts` tests share: the shards and threads of the OS,
//! and the helpers of the tests of threads that end or panic.

use std::pin::Pin;
use std::sync::mpsc;
use std::task::{Context, Poll};
use std::time::Duration;

use env::shards::{Config, Shards};
use env::thread::{Handle, Panicked};
use env::threads::Threads;

/// Asserts that `handle` joins with `outcome` in ten seconds, so a thread that does not
/// end fails its test and does not hang the run.
#[expect(clippy::disallowed_methods, reason = "the test bounds the join")]
pub(crate) fn assert_joins(handle: Handle, outcome: Result<(), Panicked>) {
    let (done, joined) = mpsc::channel();
    std::thread::spawn(move || done.send(handle.join()));
    let joined = joined.recv_timeout(Duration::from_secs(10));
    assert_eq!(joined, Ok(outcome), "the thread ends in ten seconds");
}

pub(crate) fn shards() -> Shards {
    os::shards().expect("the OS gives the cores of this process")
}

pub(crate) fn config(name: &str) -> Config {
    Config {
        name: name.into(),
        core: None,
    }
}

pub(crate) fn threads() -> Threads {
    os::threads().expect("the OS gives the cores of this process")
}

pub(crate) fn panicked(name: &str) -> Result<(), Panicked> {
    Err(Panicked { name: name.into() })
}

/// Panics when it drops.
pub(crate) struct Bomb;

impl Drop for Bomb {
    fn drop(&mut self) {
        panic!("bomb");
    }
}

/// A future that panics when it drops, and in its poll when `faulty`.
pub(crate) struct Armed {
    pub(crate) faulty: bool,
    pub(crate) _bomb: Bomb,
}

impl Future for Armed {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        assert!(!self.faulty, "body");
        Poll::Ready(())
    }
}
