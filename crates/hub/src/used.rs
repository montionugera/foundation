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

    /// Keeps the pointer of `spec`, and gives whether it differs from the one kept
    /// before.
    pub(crate) fn advance(&mut self, spec: &::mesh::used::Spec) -> bool {
        let moved = self.pointer != spec.pointer;
        self.pointer = spec.pointer;
        moved
    }
}

/// Applies the spec in use each time the watch in `state` gives one. It holds `state`
/// only during a poll, so the first poll after the state drops ends it. It also ends
/// when the group stops, after it applies the last spec in use.
pub(crate) async fn run(state: Weak<RefCell<super::State>>) {
    poll_fn(|cx| {
        loop {
            let Some(state) = state.upgrade() else {
                return Poll::Ready(());
            };
            let mut state = state.borrow_mut();
            let regional = state.region.as_mut().expect("invariant: a region");
            let applied = &mut regional.applied;
            applied.task.polled(cx);
            let Poll::Ready(next) = pin!(applied.watch.next()).poll(cx) else {
                return Poll::Pending;
            };
            // A spec can take effect just before the stop that the watch gives first.
            state.catch_up();
            if next.is_err() {
                return Poll::Ready(());
            }
        }
    })
    .await;
}
