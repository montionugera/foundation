//! A wait for a future with a deadline.

use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::task::Poll;

use env::clock::Sleep;

/// Polls `future` until it ends, or gives `None` when `sleep` fires first.
pub(crate) async fn before<T>(
    future: impl Future<Output = T>,
    mut sleep: Sleep,
) -> Option<T> {
    let mut future = pin!(future);
    poll_fn(|cx| {
        if let Poll::Ready(out) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(out));
        }
        Pin::new(&mut sleep).poll(cx).map(|()| None)
    })
    .await
}
