//! A wait for a future with a deadline.

use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::task::Poll;

use env::clock::{Clock, Sleep};
use types::time::Monotonic;

/// Polls `future` until it ends, or gives `None` at `deadline` by `clock`. It makes
/// no timer for a future that ends at its first poll.
pub(crate) async fn until<T>(
    future: impl Future<Output = T>,
    clock: &Clock,
    deadline: Monotonic,
) -> Option<T> {
    let mut future = pin!(future);
    let mut sleep: Option<Sleep> = None;
    poll_fn(|cx| {
        if let Poll::Ready(out) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(out));
        }
        let sleep = sleep.get_or_insert_with(|| clock.sleep_until(deadline));
        Pin::new(sleep).poll(cx).map(|()| None)
    })
    .await
}
