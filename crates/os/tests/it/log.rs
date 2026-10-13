//! Tests of the log dispatch of each shard and thread on real threads.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::task::yield_now;

use crate::panics::{assert_joins, config, shards, threads};
use crate::recorded::Log;

#[test]
fn each_event_goes_to_the_log_of_its_thread() {
    let (shard_log, thread_log, caller) =
        (Log::default(), Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let shard = shards()
        .logged(shard_log.dispatch())
        .start(config("a"), |tasks| {
            tracing::info!("a main");
            let spawned = Arc::new(AtomicBool::new(false));
            let ran = Arc::clone(&spawned);
            tasks.spawn(async move {
                tracing::info!("a task");
                ran.store(true, Ordering::SeqCst);
            });
            async move {
                tracing::info!("a");
                while !spawned.load(Ordering::SeqCst) {
                    yield_now().await;
                }
            }
        });
    let thread = threads().logged(thread_log.dispatch()).start("b", || {
        tracing::info!("b body");
        async { tracing::info!("b") }
    });
    let unlogged = shards().start(config("c"), |_| async { tracing::info!("c") });
    assert_joins(shard.unwrap(), Ok(()));
    assert_joins(thread.unwrap(), Ok(()));
    assert_joins(unlogged.unwrap(), Ok(()));
    assert_eq!(shard_log.messages(), ["a main", "a", "a task"]);
    assert_eq!(thread_log.messages(), ["b body", "b"]);
    assert_eq!(caller.messages(), Vec::<String>::new());
}
