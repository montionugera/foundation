//! Tests of the log dispatch of each shard and thread, in each poll and each drop.

#[path = "../../../env/tests/common/log.rs"]
mod common;

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::{shard, sim};
use crate::drivers::yield_now;
use crate::shard::Fault;
use crate::{Crash, node};
use common::Log;
use types::time::Span;

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

/// Writes one event when it drops.
struct Logs(&'static str);

impl Drop for Logs {
    fn drop(&mut self) {
        tracing::info!("{}", self.0);
    }
}

#[test]
fn a_crash_drops_each_task_with_the_log_of_its_node() {
    for seed in 0..32 {
        let mut sim = sim(seed);
        let a = sim.node(node::Config::default());
        let (log, caller) = (Log::default(), Log::default());
        let _caller = tracing::dispatcher::set_default(&caller.dispatch());
        let shards = a.shards().logged(log.dispatch());
        let _a = shards.start(shard("a"), |tasks| {
            tasks.spawn(async {
                let _guard = Logs("a task dropped");
                pending::<()>().await;
            });
            async {
                let _guard = Logs("a main dropped");
                pending::<()>().await;
            }
        });
        sim.run_for(Span::from_nanos(1_000_000)).unwrap();
        sim.crash(&a, Crash::Process);
        let mut got = log.messages();
        got.sort();
        assert_eq!(caller.messages(), Vec::<String>::new(), "seed {seed}");
        assert_eq!(got, ["a main dropped", "a task dropped"], "seed {seed}");
    }
}

#[test]
fn a_panic_drops_the_other_tasks_with_the_log_of_their_node() {
    for seed in 0..32 {
        let mut sim = sim(seed);
        let a = sim.node(node::Config::default());
        let (log, caller) = (Log::default(), Log::default());
        let _caller = tracing::dispatcher::set_default(&caller.dispatch());
        let shards = a.shards().logged(log.dispatch());
        let _a = shards.start(shard("a"), |tasks| {
            let guard = Logs("a task dropped");
            tasks.spawn(async move {
                let _guard = guard;
                pending::<()>().await;
            });
            async {
                yield_now().await;
                panic!("boom");
            }
        });
        sim.run().unwrap_err();
        assert_eq!(caller.messages(), Vec::<String>::new(), "seed {seed}");
        assert_eq!(log.messages(), ["a task dropped"], "seed {seed}");
    }
}

#[test]
fn a_drop_of_the_sim_drops_each_task_with_the_log_of_its_node() {
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    {
        let mut sim = sim(0);
        let a = sim.node(node::Config::default());
        let _a = a
            .shards()
            .logged(log.dispatch())
            .start(shard("a"), |_| async {
                let _guard = Logs("a main dropped");
                pending::<()>().await;
            });
        sim.run_for(Span::from_nanos(1_000_000)).unwrap();
    }
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a main dropped"]);
}

#[test]
fn a_failed_start_drops_its_main_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let b = sim.node(node::Config::default());
    let (a_log, b_log) = (Log::default(), Log::default());
    a.fail_shard(1, Fault::Start);
    let on_a = a.shards().logged(a_log.dispatch());
    let _b = b.threads().logged(b_log.dispatch()).start("b", move || {
        let guard = Logs("a main dropped");
        let config = env::shards::Config {
            name: "a".into(),
            core: Some(1),
        };
        let started = on_a.start(config, move |_| {
            let _guard = guard;
            async {}
        });
        let error = env::thread::Error::Start {
            name: "a".into(),
            reason: "injected".into(),
        };
        assert_eq!(started.err(), Some(error));
        async {}
    });
    sim.run().unwrap();
    assert_eq!(b_log.messages(), Vec::<String>::new());
    assert_eq!(a_log.messages(), ["a main dropped"]);
}
