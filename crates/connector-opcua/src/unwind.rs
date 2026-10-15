use std::future::poll_fn;
use std::panic::{self, AssertUnwindSafe};
use std::pin::pin;
use std::task::Poll;

/// Runs `future` to its end, and gives the panic of a poll in place of a value.
pub(crate) async fn caught<F: Future>(future: F) -> std::thread::Result<F::Output> {
    let mut future = pin!(future);
    poll_fn(|cx| {
        match panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(poll) => poll.map(Ok),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    })
    .await
}
