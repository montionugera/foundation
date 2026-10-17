//! The tasks of the client's thread, on its Tokio `LocalRuntime`.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use env::tasks::{Group, Task, Tasks};
use tokio::task::JoinSet;

/// Runs `body` with the tasks of the client's thread, then waits for each task that it
/// spawned, so that the close of a session goes out. Call it inside a `LocalRuntime`.
///
/// # Panics
///
/// Resumes the first panic of a task, as the contract of `env::tasks` asks: Tokio
/// would keep the panic in its join handle.
pub(crate) async fn scope<F: Future>(body: impl FnOnce(Tasks) -> F) -> F::Output {
    let spawned = Rc::new(Spawned::default());
    let group = Group::new(Tasks::new(Spawner(Rc::clone(&spawned))));
    let output = spawned.guard(body(group.tasks().clone())).await;
    spawned.guard(group.ended()).await;
    output
}

/// The tasks of the client's thread.
struct Spawned {
    set: RefCell<JoinSet<()>>,
    /// Wakes the future of [`Spawned::guard`] when a task starts.
    waker: Cell<Waker>,
}

impl Default for Spawned {
    fn default() -> Self {
        Self {
            set: RefCell::default(),
            waker: Cell::new(Waker::noop().clone()),
        }
    }
}

impl Spawned {
    /// Runs `body`, and resumes the first panic of a task while it runs.
    async fn guard<T>(&self, body: impl Future<Output = T>) -> T {
        let mut body = pin!(body);
        poll_fn(|cx| {
            self.check(cx);
            body.as_mut().poll(cx)
        })
        .await
    }

    /// Joins each ended task, and resumes the panic of one that panicked.
    fn check(&self, cx: &mut Context<'_>) {
        self.waker.set(cx.waker().clone());
        while let Poll::Ready(Some(joined)) = self.set.borrow_mut().poll_join_next(cx) {
            if let Err(error) = joined
                && error.is_panic()
            {
                panic::resume_unwind(error.into_panic());
            }
        }
    }
}

/// Spawns the tasks of the client's thread.
struct Spawner(Rc<Spawned>);

impl env::tasks::Driver for Spawner {
    fn spawn(&self, task: Task) {
        self.0.set.borrow_mut().spawn_local(task);
        self.0.waker.replace(Waker::noop().clone()).wake();
    }
}

#[cfg(test)]
mod tests;
