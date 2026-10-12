//! Helpers for the tests of shards and dedicated threads.

use std::panic;
use std::pin::Pin;
use std::task::{Context, Poll};

/// A panic payload whose drop panics with a `Relay` of one less, until it is 0.
pub(crate) struct Relay(pub(crate) usize);

impl Drop for Relay {
    fn drop(&mut self) {
        if self.0 > 0 {
            panic::panic_any(Self(self.0 - 1));
        }
    }
}

/// A future that is ready at its first poll, and panics with a [`Relay`] when it
/// drops.
pub(crate) struct Relayed;

impl Future for Relayed {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Ready(())
    }
}

impl Drop for Relayed {
    fn drop(&mut self) {
        panic::panic_any(Relay(2));
    }
}

/// A future that never ends, and panics with a [`Relay`] of its value when it drops.
pub(crate) struct Stuck(pub(crate) usize);

impl Future for Stuck {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        Poll::Pending
    }
}

impl Drop for Stuck {
    fn drop(&mut self) {
        panic::panic_any(Relay(self.0));
    }
}

/// The CPUs of the affinity set of the calling thread.
#[cfg(target_os = "linux")]
pub(crate) fn affinity() -> Vec<usize> {
    use rustix::thread::{CpuSet, sched_getaffinity};

    let set = sched_getaffinity(None).unwrap();
    (0..CpuSet::MAX_CPU)
        .filter(|&cpu| set.is_set(cpu))
        .collect()
}
