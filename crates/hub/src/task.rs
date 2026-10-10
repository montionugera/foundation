//! The waker of a hub task that holds the hub's state only during a poll.

use std::task::{Context, Waker};

/// The waker of a task that holds the hub's state only during a poll. A drop wakes the
/// task, which then finds the state gone and ends.
#[derive(Debug, Default)]
pub(crate) struct Waiter(Option<Waker>);

impl Waiter {
    /// Keeps the waker of `cx`. Call it at each poll of the task.
    pub(crate) fn polled(&mut self, cx: &Context<'_>) {
        if !self
            .0
            .as_ref()
            .is_some_and(|task| task.will_wake(cx.waker()))
        {
            self.0 = Some(cx.waker().clone());
        }
    }

    /// Wakes the task, once it was polled.
    pub(crate) fn wake(&self) {
        if let Some(task) = &self.0 {
            task.wake_by_ref();
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.wake();
        }
    }
}
