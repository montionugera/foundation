//! The cost of a stamp, on a sim node. Run with `cargo bench -p connector --bench
//! stamp`.
//!
//! - `now`: one read of mesh time by `Writer::now`.
//! - `request`: `Stamps::request` of a request that is ready at once, so the line is
//!   its two reads of mesh time and the rule of the stamp.
//!
//! Each figure is ns per call, with the count of the allocations of the calls.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

use connector::stamp::Stamps;
use connector::testing;
use hub::writer::Writer;
use types::authority::Authority;
use types::channel;
use types::name::Name;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const CALLS: u64 = 100_000;
const WARMUP: usize = 10;
const ROUNDS: usize = 100;

fn main() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let lines = sim.run_on(&node, |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks,
        };
        let (hub, _) = hub::testing::open(env).await;
        let connector: Name = "plant.stamp".parse().expect("a valid name");
        let first = channel::Key::from_u128(100);
        let status = testing::create_status(&connector, &[], first);
        hub.set_definitions(status.iter().map(|(name, def)| (name, def)));
        let config = hub::writer::Config {
            subject: connector,
            authority: Authority(1),
            lease: None,
            channels: vec!["plant.stamp.status.state".parse().expect("a valid name")],
        };
        let writer = hub.writer(config).await.expect("the writer opens");
        [
            time("now", || {
                black_box(writer.now());
            }),
            request(&writer),
        ]
    });
    print(&lines.expect("the run ends"));
}

/// The line `request`.
fn request(writer: &Writer) -> Line {
    let mut stamps = Stamps::default();
    let mut cx = Context::from_waker(Waker::noop());
    time("request", || {
        let request = pin!(stamps.request(writer, async { black_box(7_u8) }));
        let Poll::Ready(stamped) = request.poll(&mut cx) else {
            panic!("the request is ready")
        };
        black_box(stamped);
    })
}

/// The ns per call of each timed round, and the allocations of all of them.
struct Line {
    name: &'static str,
    nanos: Vec<u64>,
    allocations: u64,
}

/// Times `ROUNDS` rounds of `CALLS` calls of `call`, after `WARMUP` rounds.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn time(name: &'static str, mut call: impl FnMut()) -> Line {
    let mut line = Line {
        name,
        nanos: Vec::new(),
        allocations: 0,
    };
    for round in 0..WARMUP + ROUNDS {
        let start = Instant::now();
        let ((), allocations) = ALLOCATOR.count(|| {
            for _ in 0..CALLS {
                call();
            }
        });
        let nanos = Instant::now().duration_since(start).as_nanos();
        if round >= WARMUP {
            let nanos = u64::try_from(nanos).expect("a round takes under 2^64 ns");
            line.nanos.push(nanos / CALLS);
            line.allocations += allocations;
        }
    }
    line
}

/// Prints p10, p50, and p90 of the ns per call of each line, and its allocations.
#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn print(lines: &[Line]) {
    println!("ns per call over {ROUNDS} rounds of {CALLS} calls");
    println!(
        "{:<10} {:>7} {:>7} {:>7} {:>7}",
        "line", "p10", "p50", "p90", "allocs"
    );
    for line in lines {
        let mut nanos = line.nanos.clone();
        nanos.sort_unstable();
        let at = |percent: usize| nanos[nanos.len() * percent / 100];
        println!(
            "{:<10} {:>7} {:>7} {:>7} {:>7}",
            line.name,
            at(10),
            at(50),
            at(90),
            line.allocations
        );
    }
}
