//! Calls of `node` hold far less than what they must not keep.
//!
//! - `node::name` on a data directory with no file `name` and no mesh directory makes
//!   no pool for the read of a founding, whose reservation is over 2 MiB.
//! - `node::client::reach` makes two pools, one for its transport and its
//!   `program::Session` and one for the frames of the session's reads, which together
//!   reserve at most 128 MiB.
//! - A started node keeps no `os::roots::Roots` of `Config::roots`.
//! - A started node with a region keeps less than 1 MiB of the failures of its OS
//!   roots, and only their count when the first is over 1 MiB.
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
    roots();
    failures();
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

/// A node that has started on 8 MiB of roots holds less than 1 MiB more than one on
/// no roots. The first start fills what each start keeps once, so it is not counted.
fn roots() {
    let none = held(&node::Region::Absent, os::roots::Roots::default);
    let big = held(&node::Region::Absent, || os::roots::Roots {
        certificates: vec![vec![0_u8; 1 << 20]; 8],
        ..os::roots::Roots::default()
    });
    assert!(
        big < none + (1 << 20),
        "the node holds {} bytes more for 8 MiB of roots",
        big.saturating_sub(none)
    );
}

fn failures() {
    let none = held(&node::Region::Stored, os::roots::Roots::default);
    let failure =
        "failed to read PEM from file: base64 decode error: InvalidCharacter(33)";
    let many = || vec![failure.to_owned(); (8 << 20) / failure.len()];
    let failed = held(&node::Region::Stored, move || os::roots::Roots {
        failures: many(),
        ..os::roots::Roots::default()
    });
    assert!(
        failed < none + (1 << 20),
        "the node holds {} bytes more for 8 MiB of failures of its roots",
        failed.saturating_sub(none)
    );
    let unkept = held(&node::Region::Stored, move || {
        let mut failures = vec!["x".repeat((1 << 20) + 1)];
        failures.extend(many());
        os::roots::Roots {
            failures,
            ..os::roots::Roots::default()
        }
    });
    assert!(
        unkept < none + (1 << 10),
        "the node holds {} bytes more for failures of its roots that it does not keep",
        unkept.saturating_sub(none)
    );
    let none = held(&node::Region::Absent, os::roots::Roots::default);
    let unserved = held(&node::Region::Absent, move || os::roots::Roots {
        failures: many(),
        ..os::roots::Roots::default()
    });
    assert!(
        unserved < none + (1 << 10),
        "a node with no region holds {} bytes more for failures it shows to no one",
        unserved.saturating_sub(none)
    );
}

/// The bytes that a node with `region` holds once it has run for a second on the
/// roots that `roots` gives, after a first such run.
fn held(region: &node::Region, roots: impl Fn() -> os::roots::Roots) -> usize {
    let run = || {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = sim.node(sim::node::Config::default());
        let before = ALLOCATOR.held();
        let node = node::Node::start(config(&host, region.clone(), roots()));
        assert_eq!(sim.run_for(Span::SECOND), Ok(()), "the node runs");
        let held = ALLOCATOR.held().saturating_sub(before);
        node.stop();
        assert_eq!(sim.run(), Ok(()), "the node stops");
        assert_eq!(node.join(), Ok(()), "the node ends");
        held
    };
    run();
    run()
}

/// The config of a node on `host` that listens on port 7000.
fn config(
    host: &sim::node::Node,
    region: node::Region,
    roots: os::roots::Roots,
) -> node::Config<block::Heap> {
    let files = host.clone();
    node::Config {
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
        listen: SocketAddr::new(host.addresses()[0], 7000),
        region,
        name: "edge".parse().expect("a name"),
        settings: node::Settings::default(),
        vars: std::collections::BTreeMap::new(),
        roots,
        log: node::Log {
            writer: Box::new(std::io::sink()),
            format: ops::Log::default(),
        },
        threads: host.threads(),
    }
}

/// `reach` on a data directory whose node crashed, so the dial gets no answer.
fn reach() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    let listen = SocketAddr::new(host.addresses()[0], 7000);
    let node = node::Node::start(config(
        &host,
        node::Region::Stored,
        os::roots::Roots::default(),
    ));
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
