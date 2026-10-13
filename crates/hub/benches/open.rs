//! The cost of a `hub` reader open and of `Received::name`, with many channels
//! defined, on one shard of a sim node. Run with `cargo bench -p hub --bench open`.
//!
//! Each case defines `time`, `value`, and `n` more channels `w.0` to `w.<n-1>` on
//! `time`, and times these lines:
//!
//! - `timer`: an empty closure, the floor of each line's figure.
//! - `open 1/<n>`: the polls of one `Hub::reader` open of a latest reader of `value`.
//! - `open <n>/<n>`: the same, of a latest reader of `w.**`.
//! - `name <m>`: one `Received::name` for each entry of a view of a reader of `w.**`,
//!   figured per call. The frame holds `time` and up to 1,000 of `w.*`, the most that
//!   one frame of the sim home holds.
//! - `loop <n+1>`, the control of `name`: the same loop with no call.
//!
//! A debug build, which `cargo test` makes, runs one small case, so a test run stays
//! fast.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[expect(dead_code, reason = "the bench uses only some of the helpers")]
#[path = "../tests/common/mod.rs"]
mod common;
#[path = "../tests/common/node.rs"]
mod node;
mod table;

use std::hint::black_box;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use common::{SETTLE, name, unnamed};
use hub::Hub;
use hub::reader::{self, Mode, Reader};
use hub::writer;
use table::Line;
use types::authority::Authority;
use types::frame::{Form, Label, Path};
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const WARMUP: usize = 5;
const ROUNDS: usize = 40;
/// The channels of the frame of the `name` line.
const WRITTEN: usize = 1000;

fn main() {
    let cases: &[u64] = if cfg!(debug_assertions) {
        &[100]
    } else {
        &[1000, 100_000]
    };
    let (mut timer, mut lines) = (None, Vec::new());
    for &n in cases {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let (floor, case) = sim
            .run_on(&node, move |node, tasks| bench(node, tasks, n))
            .expect("the run ends");
        timer.get_or_insert(floor);
        lines.extend(case);
    }
    let timer = timer.expect("a case");
    table::print(&format!("ns per call over {ROUNDS} rounds"), &timer, &lines);
}

/// The `timer` line and the lines of the case of `n` more channels.
async fn bench(
    node: sim::node::Node,
    tasks: env::tasks::Tasks,
    n: u64,
) -> (Line, [Line; 4]) {
    let more: Vec<_> = (0..n).map(|at| name(&format!("w.{at}"))).collect();
    let hub = common::hub_alone(&node, tasks, &more).await;
    let leak = |text: String| -> &'static str { Box::leak(text.into_boxed_str()) };
    let mut timer = Line::new("timer", 1);
    let mut one = Line::new(leak(format!("open 1/{n}")), 1);
    let mut every = Line::new(leak(format!("open {n}/{n}")), 1);
    for round in 0..WARMUP + ROUNDS {
        timer.add(table::timed(&ALLOCATOR, || ()).1);
        one.add(open(&hub, &node, unnamed(&["value"], Mode::Latest)).await.1);
        every.add(open(&hub, &node, unnamed(&["w.**"], Mode::Latest)).await.1);
        for line in [&mut timer, &mut one, &mut every] {
            line.close(round >= WARMUP);
        }
    }
    let (named, looped) = names(&hub, &node, more, leak).await;
    (timer, [one, every, named, looped])
}

/// The `name` line and its control, over a view of up to [`WRITTEN`] of `more` and
/// their index.
async fn names(
    hub: &Hub,
    node: &sim::node::Node,
    more: Vec<types::name::Name>,
    leak: impl Fn(String) -> &'static str,
) -> (Line, Line) {
    let mut reader = hub
        .reader(unnamed(&["w.**"], Mode::Complete))
        .await
        .expect("opens");
    let config = writer::Config {
        subject: name("bench"),
        authority: Authority(1),
        lease: None,
        channels: more.into_iter().take(WRITTEN).collect(),
    };
    let mut writer = hub.writer(config).await.expect("opens");
    let now = writer.now().nanos().to_le_bytes();
    let set = writer.set();
    let series: Vec<_> = (0..set.entries().len()).map(|entry| (entry, 8)).collect();
    let mut draft = writer.draft(Form::Raw, &series).expect("a draft");
    for (at, entry) in set.entries().iter().enumerate() {
        draft
            .series_mut(at)
            .expect("a series")
            .copy_from_slice(&now);
        draft.set_count(entry.group, 1);
    }
    writer
        .write(Label::Path(Path::Live), draft)
        .expect("the home takes it");
    node.clock().sleep(SETTLE).await;
    let received = reader.next().await.expect("a frame");
    let entries: Vec<usize> = received.view().iter().map(|(entry, _)| entry).collect();
    let mut named = Line::new(leak(format!("name {}", entries.len())), entries.len());
    let mut looped = Line::new(leak(format!("loop {}", entries.len())), entries.len());
    for round in 0..WARMUP + ROUNDS {
        looped.add(
            table::timed(&ALLOCATOR, || {
                for &entry in &entries {
                    black_box(black_box(entry));
                }
            })
            .1,
        );
        named.add(
            table::timed(&ALLOCATOR, || {
                for &entry in &entries {
                    black_box(received.name(black_box(entry)));
                }
            })
            .1,
        );
        for line in [&mut named, &mut looped] {
            line.close(round >= WARMUP);
        }
    }
    (named, looped)
}

/// Opens `config`, with the ns and allocations of its polls only.
async fn open(
    hub: &Hub,
    node: &sim::node::Node,
    config: reader::Config,
) -> (Reader, (u64, u64)) {
    let mut open = pin!(hub.reader(config));
    let mut context = Context::from_waker(Waker::noop());
    let (mut nanos, mut allocations) = (0, 0);
    loop {
        let (poll, (more, allocated)) =
            table::timed(&ALLOCATOR, || open.as_mut().poll(&mut context));
        nanos += more;
        allocations += allocated;
        if let Poll::Ready(reader) = poll {
            return (reader.expect("opens"), (nanos, allocations));
        }
        node.clock().sleep(Span::MILLISECOND).await;
    }
}
