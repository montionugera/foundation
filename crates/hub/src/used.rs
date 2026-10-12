//! The spec task: it applies each spec that the region's mesh puts in use, so that a
//! removal ends the sessions on its channels with no open.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Weak;
use std::task::Poll;

use crate::task;

/// The spec in use that a hub with a region applied last, and the watch of the next.
#[derive(Debug)]
pub(crate) struct Applied {
    watch: ::mesh::used::Watch,
    pointer: Option<spec::Pointer>,
    /// The spec task.
    task: task::Waiter,
}

impl Applied {
    /// `spec` applied, with the watch `watch` of the specs after it.
    pub(crate) fn new(watch: ::mesh::used::Watch, spec: &::mesh::used::Spec) -> Self {
        Self {
            watch,
            pointer: spec.pointer,
            task: task::Waiter::default(),
        }
    }

    /// Keeps the pointer of `spec`, and gives whether it is not the one applied last.
    pub(crate) fn moved(&mut self, spec: &::mesh::used::Spec) -> bool {
        let moved = self.pointer != spec.pointer;
        self.pointer = spec.pointer;
        moved
    }
}

/// Applies each spec that the watch in `state` gives. It holds `state` only during a
/// poll, so the first poll after the state drops ends it. It also ends when the group
/// stops.
pub(crate) async fn run(state: Weak<RefCell<super::State>>) {
    poll_fn(|cx| {
        loop {
            let Some(state) = state.upgrade() else {
                return Poll::Ready(());
            };
            let mut state = state.borrow_mut();
            let applied = state.applied.as_mut().expect("invariant: a region");
            applied.task.polled(cx);
            let next = pin!(applied.watch.next()).poll(cx);
            match next {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(_)) => return Poll::Ready(()),
                Poll::Ready(Ok(spec)) => state.apply(&spec),
            }
        }
    })
    .await;
}
