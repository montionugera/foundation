//! A callsite that a thread with no log meets first, then a shard of a node. `tracing`
//! caches the interest of each callsite for the process, so this runs in a test binary
//! of its own, with this one test only.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use types::byte::Size;
use types::time::Span;

fn seen() {
    tracing::info!("seen");
}

/// The bytes a log writes, which each clone shares.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl io::Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lines.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The test thread, with no log, meets a callsite first, while the one live dispatch
/// is the node log: a shard of the node gets its event.
#[test]
fn a_node_log_gets_an_event_of_a_callsite_that_a_thread_with_no_log_met_first() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    let lines = Lines::default();
    let node = node::Node::start(config(&host, &lines));
    seen();
    node.spawn(|_| {
        seen();
        async {}
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    let bytes = lines.0.lock().unwrap_or_else(PoisonError::into_inner);
    let text = std::str::from_utf8(&bytes).expect("a log writes UTF-8");
    let seen = text
        .lines()
        .filter(|line| line.contains(" INFO interest: seen"));
    assert_eq!(seen.count(), 1, "{text}");
}

/// The config of a node on `host` that listens on port 7000 and logs to `lines`.
fn config(host: &sim::node::Node, lines: &Lines) -> node::Config<block::Heap> {
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
        region: node::Region::Absent,
        name: "edge".parse().expect("a name"),
        settings: node::Settings::default(),
        vars: std::collections::BTreeMap::new(),
        roots: os::roots::Roots::default(),
        log: node::Log {
            writer: Box::new(lines.clone()),
            format: ops::Log::default(),
        },
        threads: host.threads(),
    }
}
