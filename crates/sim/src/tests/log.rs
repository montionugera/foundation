//! Tests of the log dispatch of each shard and thread.

#[path = "../../../env/tests/common/log.rs"]
mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{shard, sim};
use crate::drivers::yield_now;
use crate::node;
use common::Log;

#[test]
fn each_event_goes_to_the_log_of_its_thread() {
    let mut sim = sim(7);
    let a = sim.node(node::Config::default());
    let b = sim.node(node::Config::default());
    let (shard_log, thread_log, caller) =
        (Log::default(), Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let shards = a.shards().logged(shard_log.dispatch());
    let _a = shards.start(shard("a"), |tasks| {
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
    let _b = b.threads().logged(thread_log.dispatch()).start("b", || {
        tracing::info!("b body");
        async {
            for i in 0..3 {
                tracing::info!("b {i}");
                yield_now().await;
            }
        }
    });
    let _c = a.shards().start(shard("c"), |_| async {
        tracing::info!("c");
    });
    sim.run().unwrap();
    let mut shard = shard_log.messages();
    shard.sort();
    assert_eq!(shard, ["a 0", "a 1", "a 2", "a main", "a task"]);
    assert_eq!(thread_log.messages(), ["b body", "b 0", "b 1", "b 2"]);
    assert_eq!(caller.messages(), Vec::<String>::new());
}
