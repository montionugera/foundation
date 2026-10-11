//! The commit task: at each group commit of the home, it wakes the complete readers
//! that the commit gives frames.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Weak;
use std::task::{Poll, Waker};

/// The commit task's part of the state: the commit it waits for, and its waker.
#[derive(Debug)]
pub(crate) struct Task {
    /// The next group commit of the home, which wakes the task when it resolves. It
    /// holds the ring open, so it drops with the state.
    next: home::Commit,
    /// The task's waker.
    waker: Option<Waker>,
}

impl Task {
    /// Holds `next`, the first group commit of the home that the task waits for.
    pub(crate) fn new(next: home::Commit) -> Self {
        Self { next, waker: None }
    }
}

/// Waits for each group commit of the home, then wakes the readers it holds frames
/// for. After a failed commit, it keeps the error, wakes every reader, and ends. It
/// holds `state` only during a poll, and its waker stays in the state, so the first
/// poll after the state drops ends it.
pub(crate) async fn run(state: Weak<RefCell<super::State>>) {
    poll_fn(|cx| {
        loop {
            let Some(state) = state.upgrade() else {
                return Poll::Ready(());
            };
            let mut state = state.borrow_mut();
            if !state
                .commit
                .waker
                .as_ref()
                .is_some_and(|waker| waker.will_wake(cx.waker()))
            {
                state.commit.waker = Some(cx.waker().clone());
            }
            let Poll::Ready(committed) = Pin::new(&mut state.commit.next).poll(cx)
            else {
                return Poll::Pending;
            };
            match committed {
                Ok(()) => {
                    state.wake();
                    state.commit.next = state.home.next_commit();
                }
                Err(error) => {
                    state.fail(error);
                    return Poll::Ready(());
                }
            }
        }
    })
    .await;
}

impl Drop for Task {
    /// Wakes the task, which then finds the state gone and ends.
    fn drop(&mut self) {
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}
