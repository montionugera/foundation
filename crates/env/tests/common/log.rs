//! The cases of the log of shards and threads that `os` and `sim` both pass, and a
//! `tracing` dispatch that keeps the message of each event. The tests of each include
//! this file by `#[path]`, so it uses only `env`, `std`, and `tracing`.

use std::fmt;
use std::future::{pending, poll_fn};
use std::mem;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use env::shards::{Config, Shards};
use env::thread::{Handle, Panicked};
use env::threads::Threads;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Dispatch, Event, Metadata, Subscriber};

/// The message of each event that its dispatches got, in order. Clones share them.
#[derive(Clone, Debug, Default)]
pub(crate) struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    pub(crate) fn dispatch(&self) -> Dispatch {
        Dispatch::new(self.clone())
    }

    pub(crate) fn messages(&self) -> Vec<String> {
        self.0.lock().expect("no event panics").clone()
    }
}

impl Subscriber for Log {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0.lock().expect("no event panics").push(message.0);
    }

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

/// What a case runs on.
pub(crate) struct Host {
    pub(crate) shards: Shards,
    pub(crate) threads: Threads,
    /// Runs each thread that the case started to its end.
    pub(crate) end: Box<dyn FnOnce()>,
    /// Asserts that a thread that ended joins with the outcome given.
    pub(crate) joins: fn(Handle, Result<(), Panicked>),
}

/// One `#[test]` for each case, which gives the case to `$run`. `$cases` is the name
/// of this module where the macro is called.
macro_rules! tests {
    ($cases:ident, $run:ident) => {
        $cases::tests!(
            @each $cases, $run,
            each_event_goes_to_the_log_of_its_thread,
            the_end_of_a_shard_drops_each_task_with_its_log,
        );
    };
    (@each $cases:ident, $run:ident, $($case:ident),* $(,)?) => {
        $(
            #[test]
            fn $case() {
                $run($cases::$case);
            }
        )*
    };
}
pub(crate) use tests;

/// Writes one event when it drops.
pub(crate) struct Logs(pub(crate) &'static str);

impl Drop for Logs {
    fn drop(&mut self) {
        tracing::info!("{}", self.0);
    }
}

fn config(name: &str) -> Config {
    Config {
        name: name.into(),
        core: None,
    }
}

/// Gives way to the other tasks once.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if mem::replace(&mut yielded, true) {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}

pub(crate) fn each_event_goes_to_the_log_of_its_thread(host: Host) {
    let (shard_log, thread_log, caller) =
        (Log::default(), Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let shards = host.shards.logged(shard_log.dispatch());
    let a = shards.start(config("a"), |tasks| {
        tracing::info!("a main");
        let spawned = Arc::new(AtomicBool::new(false));
        let ran = Arc::clone(&spawned);
        tasks.spawn(async move {
            tracing::info!("a task");
            ran.store(true, Ordering::SeqCst);
        });
        async move {
            for i in 0..3 {
                tracing::info!("a {i}");
                yield_now().await;
            }
            while !spawned.load(Ordering::SeqCst) {
                yield_now().await;
            }
        }
    });
    let b = host.threads.logged(thread_log.dispatch()).start("b", || {
        tracing::info!("b body");
        async {
            for i in 0..3 {
                tracing::info!("b {i}");
                yield_now().await;
            }
        }
    });
    let c = host.shards.start(config("c"), |_| async {
        tracing::info!("c");
    });
    (host.end)();
    for started in [a, b, c] {
        (host.joins)(started.expect("the thread starts"), Ok(()));
    }
    let mut shard = shard_log.messages();
    shard.sort();
    assert_eq!(shard, ["a 0", "a 1", "a 2", "a main", "a task"]);
    assert_eq!(thread_log.messages(), ["b body", "b 0", "b 1", "b 2"]);
    assert_eq!(caller.messages(), Vec::<String>::new());
}

pub(crate) fn the_end_of_a_shard_drops_each_task_with_its_log(host: Host) {
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let a = host
        .shards
        .logged(log.dispatch())
        .start(config("a"), |tasks| {
            let held = Logs("a task dropped");
            tasks.spawn(async move {
                let _held = held;
                pending::<()>().await;
            });
            async {}
        });
    (host.end)();
    (host.joins)(a.expect("the shard starts"), Ok(()));
    assert_eq!(log.messages(), ["a task dropped"]);
    assert_eq!(caller.messages(), Vec::<String>::new());
}
