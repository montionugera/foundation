//! Waits that end at a deadline.

use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll};

use env::clock::Sleep;
use types::time::Monotonic;

/// Polls `poll` until it is ready and gives its value, or gives `None` at
/// `deadline`, on `sleep`, which it resets. With no deadline, it waits without end.
pub(crate) async fn within<T>(
    sleep: &mut Sleep,
    deadline: Option<Monotonic>,
    mut poll: impl FnMut(&mut Context<'_>) -> Poll<T>,
) -> Option<T> {
    if let Some(deadline) = deadline {
        sleep.reset(deadline);
    }
    poll_fn(|cx| {
        if let Poll::Ready(value) = poll(cx) {
            return Poll::Ready(Some(value));
        }
        match deadline {
            Some(_) => Pin::new(&mut *sleep).poll(cx).map(|()| None),
            None => Poll::Pending,
        }
    })
    .await
}
