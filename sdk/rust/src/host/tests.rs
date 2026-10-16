use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::thread;

use env::clock::Driver as _;
use env::entropy::Driver as _;
use env::tasks::Tasks;
use tokio::sync::oneshot;
use types::time::Span;

use super::*;

#[test]
fn reads_from_its_epoch() {
    let clock = Clock::new();
    let now = Duration::from_nanos(clock.now().0);
    #[expect(clippy::disallowed_methods, reason = "the test reads the OS clock")]
    let after = Instant::now();
    assert!(clock.epoch() + now <= after);
    assert!(clock.epoch() + Duration::from_secs(1) > after);
}

#[test]
fn a_timer_never_fires_before_its_deadline() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let clock = env::clock::Clock::new(Clock::new());
    runtime.block_on(async {
        let mut sleep = clock.sleep(Span::MILLISECOND);
        for millis in [5, 1, 0, 7] {
            let deadline = clock.now() + Span::from_nanos(millis * 1_000_000);
            sleep.reset(deadline);
            (&mut sleep).await;
            assert!(clock.now() >= deadline, "early for {millis} ms");
        }
    });
}

#[test]
fn a_timer_whose_deadline_passed_completes_at_once() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let clock = env::clock::Clock::new(Clock::new());
    let _entered = runtime.enter();
    let mut cx = Context::from_waker(Waker::noop());
    let mut sleep = clock.sleep_until(clock.now());
    assert_eq!(Pin::new(&mut sleep).poll(&mut cx), Poll::Ready(()));
}

#[test]
fn fills_from_the_os() {
    let mut bytes = [0; 32];
    Entropy.fill(&mut bytes);
    assert_ne!(bytes, [0; 32]);
}

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
fn a_panic_in_a_task_ends_the_guarded_body() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        spawned
            .guard(async {
                tasks.spawn(async { panic!("the task failed") });
                tokio::time::sleep(Duration::from_secs(1)).await;
                unreachable!("the guard missed the panic of the task");
            })
            .await;
    });
    assert_eq!(message(&*outcome.unwrap_err()), "the task failed");
}

#[test]
fn a_task_that_ends_leaves_the_body_running() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        let (done, ended) = oneshot::channel();
        tasks.spawn(async {});
        spawned
            .guard(async {
                tasks.spawn(async { done.send(7).unwrap() });
                ended.await
            })
            .await
    });
    assert_eq!(outcome.unwrap(), Ok(7));
}

#[test]
fn a_drain_waits_for_each_task() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        let ended = Rc::new(Cell::new(false));
        let set = Rc::clone(&ended);
        tasks.spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            set.set(true);
        });
        spawned.drain().await;
        ended.get()
    });
    assert!(outcome.unwrap());
}

#[test]
fn a_drain_resumes_a_panic_of_a_task() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        tasks.spawn(async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            panic!("the task failed");
        });
        spawned.drain().await;
    });
    assert_eq!(message(&*outcome.unwrap_err()), "the task failed");
}
