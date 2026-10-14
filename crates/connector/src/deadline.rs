//! A wait for a future with a deadline.

use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::task::Poll;

use env::clock::Sleep;

/// Polls `future` until it ends, or gives `None` when `sleep` fires first. With no
/// `sleep`, it waits for `future`.
pub(crate) async fn before<T>(
    future: impl Future<Output = T>,
    mut sleep: Option<Sleep>,
) -> Option<T> {
    let mut future = pin!(future);
    poll_fn(|cx| {
        if let Poll::Ready(out) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(out));
        }
        match &mut sleep {
            Some(sleep) => Pin::new(sleep).poll(cx).map(|()| None),
            None => Poll::Pending,
        }
    })
    .await
}
