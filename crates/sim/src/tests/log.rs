//! Tests of the log dispatch of each shard and thread, in each poll and each drop.

#[path = "../../../env/tests/common/log.rs"]
mod common;

use std::cell::Cell;
use std::future::{pending, poll_fn};
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Wake, Waker};

use super::{shard, sim};
use crate::drivers::yield_now;
use crate::shard::Fault;
use crate::{Crash, line, node};
use common::{Host, Log, Logs};
use env::net::tcp;
use env::serial;
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

/// A waker of node code: it logs when it wakes and when it drops.
struct Wakes(&'static str);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        tracing::info!("{} woken", self.0);
    }
}

impl Drop for Wakes {
    fn drop(&mut self) {
        tracing::info!("{} dropped", self.0);
    }
}

/// Polls `future` once with a [`Wakes`] waker that only `future` then holds.
fn poll_with_own_waker(future: Pin<&mut impl Future>) {
    let waker = Waker::from(Arc::new(Wakes("a waker")));
    assert!(future.poll(&mut Context::from_waker(&waker)).is_pending());
}

/// Arms a sleep of `span` on `clock` with a [`Wakes`] waker, then waits forever.
async fn arm_with_own_waker(clock: env::clock::Clock, span: Span) {
    let sleep = pin!(clock.sleep(span));
    poll_with_own_waker(sleep);
    pending::<()>().await;
}

/// A drop that panics with a payload whose own drop logs.
struct PanicsInDrop;

impl Drop for PanicsInDrop {
    fn drop(&mut self) {
        std::panic::panic_any(Logs("a drop payload dropped"));
    }
}

#[test]
fn a_crash_drops_a_timer_waker_with_the_log_of_its_node() {
    for seed in 0..32 {
        let mut sim = sim(seed);
        let a = sim.node(node::Config::default());
        let (log, caller) = (Log::default(), Log::default());
        let _caller = tracing::dispatcher::set_default(&caller.dispatch());
        let clock = a.clock();
        let _a = a.shards().logged(log.dispatch()).start(shard("a"), |_| {
            arm_with_own_waker(clock, Span::from_nanos(1_000_000_000))
        });
        sim.run_for(Span::from_nanos(1_000_000)).unwrap();
        sim.crash(&a, Crash::Process);
        assert_eq!(caller.messages(), Vec::<String>::new(), "seed {seed}");
        assert_eq!(log.messages(), ["a waker dropped"], "seed {seed}");
    }
}

#[test]
fn a_drop_of_the_sim_drops_a_timer_waker_with_the_log_of_its_node() {
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    {
        let mut sim = sim(0);
        let a = sim.node(node::Config::default());
        let clock = a.clock();
        let _a = a.shards().logged(log.dispatch()).start(shard("a"), |_| {
            arm_with_own_waker(clock, Span::from_nanos(1_000_000_000))
        });
        sim.run_for(Span::from_nanos(1_000_000)).unwrap();
    }
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a waker dropped"]);
}

#[test]
fn a_panic_drops_a_timer_waker_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let clock = a.clock();
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |tasks| {
            let armed = Rc::new(Cell::new(false));
            let set = Rc::clone(&armed);
            tasks.spawn(async move {
                let sleep = pin!(clock.sleep(Span::from_nanos(1_000_000_000)));
                poll_with_own_waker(sleep);
                set.set(true);
                pending::<()>().await;
            });
            async move {
                while !armed.get() {
                    yield_now().await;
                }
                panic!("boom");
            }
        });
    sim.run().unwrap_err();
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a waker dropped"]);
}

#[test]
fn a_timer_wakes_a_waker_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let clock = a.clock();
    let _a = a.shards().logged(log.dispatch()).start(shard("a"), |_| {
        arm_with_own_waker(clock, Span::from_nanos(1_000))
    });
    sim.run_for(Span::from_nanos(1_000_000)).unwrap();
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a waker woken", "a waker dropped"]);
}

#[test]
fn the_end_of_a_file_call_wakes_its_waker_with_no_log() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let own = a.clone();
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |_| async move {
            let files = own.files();
            let free = pin!(files.free());
            poll_with_own_waker(free);
            pending::<()>().await;
        });
    sim.run_for(Span::from_nanos(1_000_000_000)).unwrap();
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), Vec::<String>::new());
}

#[test]
fn a_power_cut_drops_the_waker_of_a_listener_with_no_log() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let options = tcp::Options {
        send_buffer_bytes: 1 << 16,
        recv_buffer_bytes: 1 << 16,
        unsent_bytes_max: NonZeroUsize::new(1 << 14).unwrap(),
        delayed: false,
    };
    let listen = tcp::Listen {
        local: SocketAddr::new(a.addresses()[0], 4433),
        backlog: 4,
        options,
    };
    let mut listener = a.net().listen(&listen).unwrap();
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |_| async move {
            poll_with_own_waker(pin!(poll_fn(|cx| listener.poll_accept(cx))));
            pending::<()>().await;
        });
    sim.run_for(Span::from_nanos(1_000_000)).unwrap();
    sim.crash(&a, Crash::Power);
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), Vec::<String>::new());
}

#[test]
fn a_crash_drops_the_waker_of_a_leaked_port_with_no_log() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let b = sim.node(node::Config::default());
    let path = Path::new("/dev/ttyS0");
    sim.line(
        &a,
        path,
        &b,
        Path::new("/dev/ttyUSB0"),
        line::Config::default(),
    );
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let config = serial::Config {
        path: path.into(),
        settings: serial::Settings {
            baud: NonZeroU32::new(9_600).unwrap(),
            parity: None,
            stop_bits: serial::StopBits::One,
        },
    };
    let serial = a.serial();
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |_| async move {
            let port = Box::leak(Box::new(serial.open(&config).await.unwrap()));
            let mut buffer = [0; 8];
            poll_with_own_waker(pin!(poll_fn(|cx| port.poll_read(cx, &mut buffer))));
        });
    sim.run().unwrap();
    sim.crash(&a, Crash::Process);
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), Vec::<String>::new());
}

#[test]
fn a_panic_drops_the_payload_of_a_drop_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |tasks| {
            tasks.spawn(async {
                let _guard = PanicsInDrop;
                pending::<()>().await;
            });
            async {
                yield_now().await;
                yield_now().await;
                panic!("boom");
            }
        });
    sim.run().unwrap_err();
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a drop payload dropped"]);
}

#[test]
fn a_crash_drops_the_payload_of_a_drop_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let _a = a
        .shards()
        .logged(log.dispatch())
        .start(shard("a"), |_| async {
            let _guard = PanicsInDrop;
            pending::<()>().await;
        });
    sim.run_for(Span::from_nanos(1_000_000)).unwrap();
    let crash =
        std::panic::catch_unwind(AssertUnwindSafe(|| sim.crash(&a, Crash::Process)));
    assert!(crash.is_err());
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a drop payload dropped"]);
}

#[test]
fn a_failed_pin_drops_its_main_with_the_log_of_its_node() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let b = sim.node(node::Config::default());
    let (a_log, b_log) = (Log::default(), Log::default());
    a.fail_shard(1, Fault::Pin);
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
        let error = env::thread::Error::Pin {
            name: "a".into(),
            core: 1,
            reason: "injected".into(),
        };
        assert_eq!(started.err(), Some(error));
        async {}
    });
    sim.run().unwrap();
    assert_eq!(b_log.messages(), Vec::<String>::new());
    assert_eq!(a_log.messages(), ["a main dropped"]);
}

#[test]
fn a_crash_drops_a_start_that_has_not_run_with_its_log() {
    let mut sim = sim(0);
    let a = sim.node(node::Config::default());
    let (log, caller) = (Log::default(), Log::default());
    let _caller = tracing::dispatcher::set_default(&caller.dispatch());
    let guard = Logs("a start dropped");
    let _a = a.threads().logged(log.dispatch()).start("a", move || {
        let _guard = guard;
        async {}
    });
    sim.crash(&a, Crash::Process);
    assert_eq!(caller.messages(), Vec::<String>::new());
    assert_eq!(log.messages(), ["a start dropped"]);
}

/// Starts a thread with the log `1` on `0` when it drops.
struct Starts(env::threads::Threads, Log);

impl Drop for Starts {
    fn drop(&mut self) {
        let held = Logs("a start dropped");
        let started = self.0.logged(self.1.dispatch()).start("c", move || {
            let _held = held;
            async {}
        });
        drop(started);
    }
}

#[test]
fn a_start_in_the_drop_of_a_crash_drops_with_its_own_log() {
    for seed in 0..32 {
        let mut sim = sim(seed);
        let a = sim.node(node::Config::default());
        let (log, other, caller) = (Log::default(), Log::default(), Log::default());
        let _caller = tracing::dispatcher::set_default(&caller.dispatch());
        let starts = Starts(a.threads(), other.clone());
        let _a = a
            .shards()
            .logged(log.dispatch())
            .start(shard("a"), |_| async {
                let _starts = starts;
                pending::<()>().await;
            });
        sim.run_for(Span::from_nanos(1_000_000)).unwrap();
        sim.crash(&a, Crash::Process);
        assert_eq!(caller.messages(), Vec::<String>::new(), "seed {seed}");
        assert_eq!(log.messages(), Vec::<String>::new(), "seed {seed}");
        assert_eq!(other.messages(), ["a start dropped"], "seed {seed}");
    }
}
