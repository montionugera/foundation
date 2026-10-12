//! The registration of an open that waits, which a change of one of its channels
//! wakes.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

use types::channel::Key;

use crate::{Ending, State};

/// An open that waits, registered on its channels: a call that removes or changes one
/// of them ends its wait at once. Dropping it ends the registration.
pub(crate) struct Pending<'s> {
    state: &'s Rc<RefCell<State>>,
    key: u64,
    ending: Ending,
}

impl<'s> Pending<'s> {
    /// An open registered on no channel.
    pub(crate) fn new(state: &'s Rc<RefCell<State>>) -> Self {
        let mut borrowed = state.borrow_mut();
        let key = borrowed.opened;
        borrowed.opened += 1;
        let ending = borrowed.opens.add(key, Box::default());
        drop(borrowed);
        Self { state, key, ending }
    }

    /// Registers the open on `keys` alone, also after a change ended its wait.
    pub(crate) fn register(&mut self, keys: Box<[Key]>) {
        let mut state = self.state.borrow_mut();
        state.opens.remove(self.key);
        self.ending = state.opens.add(self.key, keys);
    }

    /// Adds `keys` to the channels that the open is registered on.
    ///
    /// # Panics
    ///
    /// When a change ended the wait and no [`Self::register`] came after it.
    pub(crate) fn extend(&self, keys: &[Key]) {
        let mut state = self.state.borrow_mut();
        state.opens.keys_mut(self.key).extend_from_slice(keys);
    }

    /// Gives what `future` gives, or, at once, the first channel of the open that a
    /// call removed or changed.
    pub(crate) async fn wait<T>(
        &self,
        future: impl Future<Output = T>,
    ) -> Result<T, Key> {
        let mut future = pin!(future);
        poll_fn(|cx| {
            if let Some(key) = self.ending.removed() {
                return Poll::Ready(Err(key));
            }
            let mut state = self.state.borrow_mut();
            state.waiting.insert(self.key, cx.waker().clone());
            drop(state);
            future.as_mut().poll(cx).map(Ok)
        })
        .await
    }

    /// Ends the registration, and gives its channels, in order.
    ///
    /// # Panics
    ///
    /// As [`Self::extend`].
    pub(crate) fn into_keys(self) -> Vec<Key> {
        self.state.borrow_mut().opens.take(self.key)
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.opens.remove(self.key);
        state.waiting.remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::pin::pin;
    use std::task::{Context, Waker};

    use super::Pending;

    /// An open leaves no entry in the hub once it drops, also when it set its waker.
    /// It reads private maps, as no public call reads them: a leak is only held
    /// memory, and a counting allocator needs a binary with no harness that serves
    /// streams over the sim network.
    #[test]
    fn an_open_keeps_no_entry_once_it_drops() {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let ran = sim.run_on(&node, |node, tasks| async move {
            let (hub, _) = crate::testing::open(crate::testing::Env {
                files: node.files(),
                clock: node.clock(),
                wall: node.wall(),
                entropy: node.entropy(),
                tasks,
            })
            .await;
            let entries = || {
                let state = hub.0.borrow();
                (state.opens.0.len(), state.waiting.len())
            };
            let opening = Pending::new(&hub.0);
            let cx = &mut Context::from_waker(Waker::noop());
            let waited = pin!(opening.wait(pending::<()>())).poll(cx);
            assert!(waited.is_pending());
            assert_eq!(entries(), (1, 1));
            drop(opening);
            assert_eq!(entries(), (0, 0));
        });
        assert_eq!(ran, Ok(()));
    }
}
