//! The gate task: at the end of each control lease and grace, it lets the home record
//! the handoff, so that a holder that stops writing loses control with no other call.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Weak;
use std::task::{Poll, Waker};

use types::time::Monotonic;

/// The gate task's part of the state: its waker.
#[derive(Debug, Default)]
pub(crate) struct Signal {
    task: Option<Waker>,
}

impl Signal {
    /// Notes a home call that may make the home's deadline earlier: a writer open or
    /// close. Wakes the task.
    pub(crate) fn changed(&self) {
        if let Some(task) = &self.task {
            task.wake_by_ref();
        }
    }
}

/// Sleeps until the home's deadline, advances the home, and notes the append for the
/// commit task. It holds `state` only during a poll, so the first poll after the state
/// drops ends it.
pub(crate) async fn run(state: Weak<RefCell<super::State>>, time: clock::Reader) {
    let mut sleep = time.sleep_until(Monotonic::default());
    poll_fn(|cx| {
        loop {
            let Some(state) = state.upgrade() else {
                return Poll::Ready(());
            };
            let mut state = state.borrow_mut();
            if !state
                .gate
                .task
                .as_ref()
                .is_some_and(|task| task.will_wake(cx.waker()))
            {
                state.gate.task = Some(cx.waker().clone());
            }
            let Some(deadline) = state.home.deadline() else {
                return Poll::Pending;
            };
            sleep.reset(deadline);
            if Pin::new(&mut sleep).poll(cx).is_pending() {
                return Poll::Pending;
            }
            // Each deadline after the advance is later than now.
            state.home.advance();
            state.commit.appended();
        }
    })
    .await;
}

impl Drop for Signal {
    /// Wakes the task, which then finds the state gone and ends.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.wake();
        }
    }
}
