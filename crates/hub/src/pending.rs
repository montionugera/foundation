//! The registration of an open that waits, which a change of one of its channels
//! wakes.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

use types::channel::Key;

use crate::{Ending, State};

/// An open that waits, registered on its channels: a change of the definitions that
/// removes or changes one of them ends its wait at once. Dropping it ends the
/// registration.
pub(crate) struct Pending<'s> {
    state: &'s Rc<RefCell<State>>,
    key: u64,
    ending: Ending,
}

impl<'s> Pending<'s> {
    /// An open registered on `keys`.
    pub(crate) fn new(state: &'s Rc<RefCell<State>>, keys: Box<[Key]>) -> Self {
        let mut borrowed = state.borrow_mut();
        let key = borrowed.opened;
        borrowed.opened += 1;
        let ending = borrowed.opens.add(key, keys);
        drop(borrowed);
        Self { state, key, ending }
    }

    /// Registers the open on `keys` too. After a change of the definitions ended its
    /// wait, it does nothing, and the next wait gives that change.
    pub(crate) fn extend(&self, keys: &[Key]) {
        self.state.borrow_mut().opens.extend(self.key, keys);
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
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.opens.remove(self.key);
        state.waiting.remove(&self.key);
    }
}
