//! The gate task: at the end of each control lease and grace, it lets the home record
//! the handoff, so that a holder that stops writing loses control with no other call.

use std::cell::RefCell;
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Weak;
use std::task::Poll;

use types::time::Monotonic;

/// Sleeps until the home's deadline, then advances the home. It holds `state` only
/// during a poll, so the first poll after the state drops ends it.
pub(crate) async fn run(state: Weak<RefCell<super::State>>, time: clock::Reader) {
    let mut sleep = time.sleep_until(Monotonic::default());
    poll_fn(|cx| {
        loop {
            let Some(state) = state.upgrade() else {
                return Poll::Ready(());
            };
            let mut state = state.borrow_mut();
            state.gate.polled(cx);
            let Some(deadline) = state.home.deadline() else {
                return Poll::Pending;
            };
            sleep.reset(deadline);
            if Pin::new(&mut sleep).poll(cx).is_pending() {
                return Poll::Pending;
            }
            // Each deadline after the advance is later than now.
            state.home.advance();
        }
    })
    .await;
}
