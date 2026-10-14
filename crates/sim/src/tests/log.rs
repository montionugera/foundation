//! Tests of the log dispatch of each shard and thread, in each poll and each drop.

#[path = "../../../env/tests/common/log.rs"]
mod common;

use std::future::pending;

use super::{shard, sim};
use crate::drivers::yield_now;
use crate::shard::Fault;
use crate::{Crash, node};
use common::{Host, Log, Logs};
use types::time::Span;

fn run(case: fn(Host)) {
    let mut sim = sim(7);
    let node = sim.node(node::Config::default());
    case(Host {
        shards: node.shards(),
        threads: node.threads(),
        end: Box::new(move || sim.run().expect("the run ends")),
        joins: |handle, outcome| assert_eq!(handle.join(), outcome),
    });
}

common::tests!(common, run);

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
fn a_panic_drops_its_payload_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let shards = a.shards().logged(log.dispatch());
    let _a = shards.start(shard("a"), |_| async {
        std::panic::panic_any(Logs("a payload dropped"));
    });
    sim.run().unwrap_err();
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a payload dropped"]);
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
