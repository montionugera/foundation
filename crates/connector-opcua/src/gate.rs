//! A gate for tests that stops a future, such as a server, and lets it go on later.

use std::cell::RefCell;
use std::future;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Poll, Waker};

/// A gate that [`Gate::hold`] polls a future through. A clone shares the gate.
#[derive(Clone, Debug, Default)]
pub(crate) struct Gate(Rc<RefCell<State>>);

#[derive(Debug, Default)]
struct State {
    closed: bool,
    /// The waker of the held future while the gate is closed.
    waker: Option<Waker>,
}

impl Gate {
    /// Stops each poll of the held future until [`Gate::open`]. A server that it
    /// holds then answers no request, also one that it already got.
    pub(crate) fn close(&self) {
        self.0.borrow_mut().closed = true;
    }

    /// Lets the held future go on after [`Gate::close`].
    pub(crate) fn open(&self) {
        let mut state = self.0.borrow_mut();
        state.closed = false;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }

    /// Polls `future` only while the gate is open, and gives its output.
    pub(crate) async fn hold<F: Future>(&self, future: F) -> F::Output {
        let mut future = pin!(future);
        future::poll_fn(|cx| {
            {
                let mut state = self.0.borrow_mut();
                if state.closed {
                    state.waker = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
            future.as_mut().poll(cx)
        })
        .await
    }
}
