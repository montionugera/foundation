//! Values of node code that the sim keeps past the poll that gave them.

use std::task;

use tracing::Dispatch;

/// A waker of a net, serial, or file event, with the log of the poll that gave it.
pub(crate) type Waker = Kept<task::Waker>;

/// A value of node code with the log of the poll that gave it. It drops, and a
/// [`Waker`] wakes, with that log as the default dispatch, wherever the sim lets go
/// of it.
pub(crate) struct Kept<T> {
    /// Some until the drop.
    value: Option<T>,
    log: Dispatch,
}

impl<T> Kept<T> {
    /// Keeps `value` with the default dispatch of the calling thread, which is the
    /// log of the node thread that the sim polls.
    pub(crate) fn new(value: T) -> Self {
        Self::logged(value, tracing::dispatcher::get_default(Dispatch::clone))
    }

    /// Keeps `value` with `log`.
    pub(crate) fn logged(value: T, log: Dispatch) -> Self {
        Self {
            value: Some(value),
            log,
        }
    }

    /// Gives the value back, to drop or use in the log of the caller.
    pub(crate) fn into_inner(mut self) -> T {
        (self.value.take()).expect("invariant: a kept value is there until the drop")
    }
}

impl Kept<task::Waker> {
    /// A waker that wakes nothing, with no log.
    pub(crate) fn noop() -> Self {
        Self::logged(task::Waker::noop().clone(), Dispatch::none())
    }

    /// Whether this waker and `other` wake the same task.
    pub(crate) fn will_wake(&self, other: &task::Waker) -> bool {
        self.value
            .as_ref()
            .is_some_and(|waker| waker.will_wake(other))
    }

    /// Wakes the task, and drops the waker, with its log.
    pub(crate) fn wake(mut self) {
        let waker = self.value.take();
        tracing::dispatcher::with_default(&self.log, || waker.map(task::Waker::wake));
    }
}

impl<T> Drop for Kept<T> {
    fn drop(&mut self) {
        let value = self.value.take();
        tracing::dispatcher::with_default(&self.log, || drop(value));
    }
}
