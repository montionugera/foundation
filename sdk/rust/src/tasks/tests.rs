use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::thread;
use std::time::Duration;

use tokio::sync::oneshot;

use super::*;

/// Runs `body` on a local runtime, and gives its panic.
fn local<T>(body: impl Future<Output = T>) -> thread::Result<T> {
    let runtime = runtime();
    panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(body)))
}

fn runtime() -> tokio::runtime::LocalRuntime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build_local(tokio::runtime::LocalOptions::default())
        .unwrap()
}

fn message(payload: &(dyn Any + Send)) -> &str {
    payload.downcast_ref::<&str>().copied().unwrap_or_default()
}

#[test]
fn a_panic_of_a_task_ends_a_body_that_never_wakes() {
    let outcome = local(async {
        let mut body = pin!(scope(|tasks| async move {
            tasks.spawn(async { panic!("the task failed") });
            std::future::pending::<()>().await;
        }));
        let mut limit = pin!(tokio::time::sleep(Duration::from_secs(1)));
        poll_fn(|cx| {
            assert!(
                limit.as_mut().poll(cx).is_pending(),
                "the scope missed the wake"
            );
            body.as_mut().poll(cx)
        })
        .await;
    });
    assert_eq!(message(&*outcome.unwrap_err()), "the task failed");
}

#[test]
fn a_task_that_ends_leaves_the_body_running() {
    let outcome = local(scope(|tasks| async move {
        let (done, ended) = oneshot::channel();
        tasks.spawn(async {});
        tasks.spawn(async { done.send(7).unwrap() });
        ended.await
    }));
    assert_eq!(outcome.unwrap(), Ok(7));
}

#[test]
fn a_scope_waits_for_each_task_after_its_body() {
    let ended = Rc::new(Cell::new(false));
    let set = Rc::clone(&ended);
    let outcome = local(scope(|tasks| async move {
        tasks.spawn(async move {
            tokio::task::yield_now().await;
            set.set(true);
        });
    }));
    outcome.unwrap();
    assert!(ended.get());
}

#[test]
fn a_scope_resumes_a_panic_of_a_task_after_its_body() {
    let outcome = local(scope(|tasks| async move {
        tasks.spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            panic!("the task failed");
        });
    }));
    assert_eq!(message(&*outcome.unwrap_err()), "the task failed");
}
