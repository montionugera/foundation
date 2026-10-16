//! Tests of thread faults through `env::threads`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use env::thread;

use super::{shard, sim};
use crate::node;

fn injected(name: &str) -> thread::Error {
    thread::Error::Start {
        name: name.into(),
        reason: "injected".into(),
    }
}

#[test]
fn a_fault_fails_only_the_next_start_of_its_name_on_its_node() {
    let mut sim = sim(0);
    let other = sim.node(node::Config::default());
    let node = sim.node(node::Config::default());
    node.fail_thread("log");
    let threads = node.threads();
    let ran = Arc::new(AtomicBool::new(false));
    let beside = threads.start("logs", || async {});
    let elsewhere = other.threads().start("log", || async {});
    let body = Arc::clone(&ran);
    let e = threads
        .start(
            "log",
            move || async move { body.store(true, Ordering::Relaxed) },
        )
        .unwrap_err();
    assert_eq!(e, injected("log"));
    assert_eq!(e.to_string(), "cannot start thread log: injected");
    let again = threads.start("log", || async {});
    sim.run().unwrap();
    assert!(!ran.load(Ordering::Relaxed));
    beside.unwrap().join().unwrap();
    elsewhere.unwrap().join().unwrap();
    again.unwrap().join().unwrap();
}

#[test]
fn faults_on_one_name_fire_in_turn() {
    let mut sim = sim(0);
    let node = sim.node(node::Config::default());
    node.fail_thread("log");
    node.fail_thread("log");
    let threads = node.threads();
    let first = threads.start("log", || async {});
    let second = threads.start("log", || async {});
    let third = threads.start("log", || async {});
    assert_eq!(first.unwrap_err(), injected("log"));
    assert_eq!(second.unwrap_err(), injected("log"));
    sim.run().unwrap();
    third.unwrap().join().unwrap();
}

#[test]
fn a_start_takes_the_fault_of_its_name_from_the_middle_of_the_faults() {
    let mut sim = sim(0);
    let node = sim.node(node::Config::default());
    node.fail_thread("a");
    node.fail_thread("b");
    node.fail_thread("c");
    let threads = node.threads();
    assert_eq!(threads.start("b", || async {}).unwrap_err(), injected("b"));
    assert_eq!(threads.start("a", || async {}).unwrap_err(), injected("a"));
    assert_eq!(threads.start("c", || async {}).unwrap_err(), injected("c"));
    let again = ["a", "b", "c"].map(|name| threads.start(name, || async {}));
    sim.run().unwrap();
    for started in again {
        started.unwrap().join().unwrap();
    }
}

#[test]
fn a_shard_with_the_name_of_a_faulted_thread_starts() {
    let mut sim = sim(0);
    let node = sim.node(node::Config::default());
    node.fail_thread("log");
    let started = node.shards().start(shard("log"), |_| async {});
    let failed = node.threads().start("log", || async {});
    assert_eq!(failed.unwrap_err(), injected("log"));
    sim.run().unwrap();
    started.unwrap().join().unwrap();
}
