use clock::{Clock, Reader, Time};
use estimate::Measurement;
use sim::Sim;
use sim::node::{self, Node};
use types::time::{Interval, Monotonic, Span};

/// The error of an unknown measurement.
pub(crate) const UNKNOWN: Span = Measurement::unknown(Monotonic(0), Span::ZERO).error();

pub(crate) fn ms(n: i64) -> Span {
    Span::from_nanos(n * 1_000_000)
}

/// A simulated node whose OS gives a bound of 10 ms.
pub(crate) fn node() -> (Sim, Node) {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(node::Config::default());
    (sim, node)
}

/// The [`Time`] that a reader on `node` gives now, with `mesh`. The sim clock does not
/// move between reads.
pub(crate) fn time(node: &Node, mesh: Option<Interval>) -> Time {
    Time {
        monotonic: node.clock().now(),
        mesh,
    }
}

/// Runs a clock on one shard of `host`, fed by `wall`.
pub(crate) fn run(host: &Node, wall: env::wall::Wall) -> Reader {
    let (clock, reader) = Clock::new(host.clock());
    start(host, clock, wall);
    reader
}

/// Runs `clock` on one shard of `host`, fed by `wall`.
pub(crate) fn start(host: &Node, clock: Clock, wall: env::wall::Wall) {
    let shard = env::shards::Config {
        name: "shard-0".into(),
        core: Some(0),
    };
    let handle = host
        .shards()
        .start(shard, |_tasks| async { clock.run(wall).await });
    drop(handle.expect("the shard starts"));
}
