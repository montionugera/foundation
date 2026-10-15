//! Two calls of `node` hold far less than a pool that they must not make.
//!
//! - `node::name` on a data directory with no file `name` and no mesh directory makes
//!   no pool for the read of a founding, whose reservation is over 2 MiB.
//! - `node::client::reach` makes one pool for its transport and its
//!   `program::Session`, which reserves at most 128 MiB.
//!
//! The count covers each thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::net::SocketAddr;

use types::byte::Size;
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The most bytes that `node::name` may hold at once: far under the reservation of the
/// pool of the founding read, and over what the call holds when it finds no file.
const NAME_BOUND: usize = 64 << 10;

/// The most bytes that `node::client::reach` may hold at once.
const REACH_BOUND: usize = 1 << 27;

fn main() {
    name();
    reach();
}

/// The peak of the bytes held while `run` runs, over those held before it.
fn peak<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATOR.held();
    ALLOCATOR.reset_peak();
    let out = run();
    (out, ALLOCATOR.peak() - before)
}

fn name() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    // The first run makes the allocations that a sim makes once.
    assert_eq!(
        sim.run_on(&host, |_, _| async {}),
        Ok(()),
        "the first run ends"
    );
    let (named, peak) = peak(|| {
        sim.run_on(&host, |host, _| async move {
            node::name(&host.files(), None).await
        })
    });
    assert_eq!(
        named,
        Ok(Err(node::Error::Unnamed)),
        "an empty data directory holds no name"
    );
    assert!(peak < NAME_BOUND, "`name` held {peak} bytes at once");
}

/// `reach` on a data directory whose node crashed, so the dial gets no answer.
fn reach() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    let listen = SocketAddr::new(host.addresses()[0], 7000);
    let files = host.clone();
    let node = node::Node::start(node::Config {
        shards: host.shards(),
        clock: host.clock(),
        wall: host.wall(),
        budget: node::Budget {
            pool: Size::MEBIBYTE,
            disk: Size::from_bytes(2 * (8192 + (64 << 20))),
        },
        memory: Box::new(|len| Ok(block::Heap::new(len))),
        files: Box::new(move || {
            let files = files.clone();
            Box::new(move || files.files())
        }),
        entropy: host.entropy(),
        net: host.net(),
        listen,
        region: node::Region::Stored,
        name: "edge".parse().expect("a name"),
        settings: node::Settings::default(),
        vars: std::collections::BTreeMap::new(),
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()), "the node runs");
    sim.crash(&host, sim::Crash::Process);
    drop(node);
    let (reached, peak) = peak(|| {
        sim.run_on(&host, |host, tasks| async move {
            let reached = node::client::reach(node::client::Config {
                files: host.files(),
                net: host.net(),
                clock: host.clock(),
                entropy: host.entropy(),
                tasks,
            })
            .await;
            reached.map(drop)
        })
    });
    let Ok(Err(node::client::Error::Unanswered { at, .. })) = reached else {
        panic!("a crashed node answers no dial: {reached:?}");
    };
    assert_eq!(at, listen, "`reach` dials the address of the node");
    assert!(peak <= REACH_BOUND, "`reach` held {peak} bytes at once");
}
