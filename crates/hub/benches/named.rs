//! The cost of the position records of named complete readers on one shard of a sim
//! node with no hub, as the count of held records grows. Run with
//! `cargo bench -p hub --bench named`.
//!
//! For each count N, a shard carries N indexes, each with one named complete reader
//! that closed and holds its record for an hour, and one spare index. Each round opens
//! a named complete reader on the spare index, untimed, and times these lines:
//!
//! - `timer`: an empty closure, the floor of each line's figure.
//! - `tick N held`: `advance` after 1.1 s of sim time, with the spare reader open.
//! - `close N held`: `close_reader` of the spare reader, which appends its record.
//! - `carry N held`: `carry` of a new index.
//!
//! Judge a figure by `net`, its p50 less the floor's.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[path = "../tests/common/node.rs"]
mod node;
mod table;

use home::reader::complete::Charge;
use home::reader::named;
use table::Line;
use types::channel;
use types::frame::key_set::{Entry, Group, Interner};
use types::name::Name;
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

/// The counts of held records, each with the names of its lines.
const HELD: [(usize, [&str; 3]); 3] = [
    (1, ["tick 1 held", "close 1 held", "carry 1 held"]),
    (100, ["tick 100 held", "close 100 held", "carry 100 held"]),
    (
        1000,
        ["tick 1000 held", "close 1000 held", "carry 1000 held"],
    ),
];
const WARMUP: usize = 10;
const ROUNDS: usize = 100;
/// The hold of each record, longer than the run.
const HOUR: Span = Span::from_nanos(3_600_000_000_000);

fn main() {
    let (mut timer, mut lines) = (None, Vec::new());
    for (held, names) in HELD {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let run = move |node, tasks| bench(node, tasks, held, names);
        let (floor, held) = sim.run_on(&node, run).expect("the run ends");
        timer.get_or_insert(floor);
        lines.extend(held);
    }
    let timer = timer.expect("a count of held records");
    table::print(&format!("ns per call over {ROUNDS} rounds"), &timer, &lines);
}

/// The `timer` line and the lines named `names` of a shard with `held` held records.
async fn bench(
    node: sim::node::Node,
    tasks: env::tasks::Tasks,
    held: usize,
    names: [&'static str; 3],
) -> (Line, [Line; 3]) {
    let clock = node.clock();
    let (mut shard, mut interner, _, _) =
        home::testing::shard(node::env(&node, tasks)).await;
    let mut indexes = indexes(&mut interner, held + 1 + WARMUP + ROUNDS).into_iter();
    for (at, (slot, key)) in indexes.by_ref().take(held).enumerate() {
        assert!(shard.carry(slot, key), "each index is new");
        let opened = shard
            .open_named_complete(slot, reader(at), HOUR, u64::MAX, Charge::Whole)
            .expect("opens");
        shard.close_reader(opened.key.into());
        if at % 50 == 49 {
            shard.committed().await.expect("commits");
        }
    }
    let (spare, key) = indexes.next().expect("a spare index");
    assert!(shard.carry(spare, key), "the spare index is new");
    shard.committed().await.expect("commits");
    let [tick, close, carry] = names;
    let mut timer = Line::new("timer", 1);
    let mut lines = [tick, close, carry].map(|name| Line::new(name, 1));
    for (round, (slot, key)) in indexes.enumerate() {
        let opened = shard
            .open_named_complete(spare, reader(held), HOUR, u64::MAX, Charge::Whole)
            .expect("opens");
        clock.sleep(Span::from_nanos(1_100_000_000)).await;
        timer.add(table::timed(&ALLOCATOR, || ()).1);
        lines[0].add(table::timed(&ALLOCATOR, || shard.advance()).1);
        let close = || shard.close_reader(opened.key.into());
        lines[1].add(table::timed(&ALLOCATOR, close).1);
        let (carried, cost) = table::timed(&ALLOCATOR, || shard.carry(slot, key));
        assert!(carried, "each index is new");
        lines[2].add(cost);
        shard.committed().await.expect("commits");
        timer.close(round >= WARMUP);
        for line in &mut lines {
            line.close(round >= WARMUP);
        }
    }
    (timer, lines)
}

/// The slot and key of each of `count` new indexes with no data channel.
fn indexes(
    interner: &mut Interner,
    count: usize,
) -> Vec<(channel::Slot, channel::Key)> {
    let key = |n: usize| channel::Key::from_u128(u128::try_from(n + 1).expect("few"));
    let groups: Vec<_> = (0..count)
        .map(|n| Group {
            index: key(n),
            data: &[],
        })
        .collect();
    let set = interner.intern(&groups);
    set.groups()
        .iter()
        .map(|&entry| {
            let Entry { slot, key, .. } = set.entries()[entry];
            (slot, key)
        })
        .collect()
}

/// The named reader `n`, with the longest subject and name.
fn reader(n: usize) -> named::Key {
    named::Key {
        subject: "s"
            .repeat(Name::MAX_BYTES)
            .parse()
            .expect("a valid subject"),
        name: format!("{n:0>255}").parse().expect("a valid name"),
    }
}
