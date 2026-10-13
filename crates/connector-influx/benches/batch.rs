//! The cost of the path from a reader to the body of an InfluxDB request, on one shard
//! of a sim node. Run with `cargo bench -p connector-influx --bench batch`.
//!
//! For each shape, a writer writes frames of an index and `channels` `f64` channels,
//! with `samples` sample times each, and a complete reader reads them. Each round adds
//! `FRAMES` frames to one batch with `Batch::add`, then takes its body with
//! `Batch::take`, as the send loop does for one request. Only these two calls are
//! timed. The figures are per frame: the ns of each round, at percentiles over the
//! rounds, the ns for each sample of a channel at p50, and the allocations.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::time::Instant;

use connector::kind::Table;
use connector::testing;
use connector_influx::bench::Batch;
use hub::reader::Mode;
use hub::writer::Writer;
use spec::channel::{Channel, Data, Kind};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::authority::Authority;
use types::channel;
use types::frame::{Form, Label, Path};
use types::name::{Name, Selector};
use types::sample::{Scalar, Type};
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

/// Frames per round, and so per batch.
const FRAMES: usize = 16;
const WARMUP: usize = 2;
/// Few enough that each frame of each shape fits the ring of the sim hub, which no ack
/// frees.
const ROUNDS: usize = 20;
/// Each `(channels, samples)`.
const SHAPES: [(usize, usize); 6] =
    [(1, 1), (8, 1), (64, 1), (1, 100), (8, 100), (64, 10)];

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    println!("ns and allocations per frame over {ROUNDS} rounds of {FRAMES} frames");
    println!(
        "{:>8} {:>7} {:>9} {:>9} {:>9} {:>11} {:>12} {:>11}",
        "channels",
        "samples",
        "p10",
        "p50",
        "p90",
        "ns/sample",
        "allocs/frame",
        "body bytes"
    );
    for (channels, samples) in SHAPES {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let run = move |node, tasks| bench(node, tasks, channels, samples);
        let (mut nanos, allocations, body) =
            sim.run_on(&node, run).expect("the run ends");
        nanos.sort_unstable();
        let at = |percent: usize| nanos[nanos.len() * percent / 100];
        let frames = ROUNDS * FRAMES;
        println!(
            "{channels:>8} {samples:>7} {:>9} {:>9} {:>9} {:>11.2} {:>12.2} {body:>11}",
            at(10),
            at(50),
            at(90),
            per(at(50)) / per(channels * samples),
            per(allocations) / per(frames),
        );
    }
}

/// The ns per frame of each timed round, the allocations of all timed rounds, and the
/// bytes of the last body.
async fn bench(
    node: sim::node::Node,
    tasks: env::tasks::Tasks,
    channels: usize,
    samples: usize,
) -> (Vec<u64>, u64, usize) {
    let env = hub::testing::Env {
        files: node.files(),
        clock: node.clock(),
        wall: node.wall(),
        entropy: node.entropy(),
        tasks,
    };
    let hub = testing::create_config(env, node.net(), Table::new())
        .await
        .hub;
    let names = define(&hub, channels);
    let config = hub::writer::Config {
        subject: name("bench.writer"),
        authority: Authority::ABSOLUTE,
        lease: None,
        channels: names,
    };
    let mut writer = hub.writer(config).await.expect("the writer opens");
    let config = hub::reader::Config {
        select: Selector::new(["edge.**"]).expect("a selector"),
        mode: Mode::Complete,
        subject: name("bench.reader"),
        name: None,
        hold: Span::ZERO,
    };
    let mut reader = hub.reader(config).await.expect("the reader opens");
    let mut batch = Batch::new("telemetry".into());
    let (mut nanos, mut allocations, mut body) = (Vec::new(), 0, 0);
    let mut stamp = writer.now().nanos();
    for round in 0..WARMUP + ROUNDS {
        for _ in 0..FRAMES {
            write(&mut writer, stamp, samples);
            stamp += i64::try_from(samples).expect("few");
        }
        node.clock().sleep(Span::SECOND).await;
        let mut figures = (0, 0);
        for _ in 0..FRAMES {
            let received = reader.next().await.expect("a frame");
            let (added, (ns, allocs)) = timed(|| batch.add(&received));
            added.expect("each channel holds f64");
            figures = (figures.0 + ns, figures.1 + allocs);
            let position = received.position();
            reader.ack(position);
        }
        let (taken, (ns, allocs)) = timed(|| batch.take());
        body = taken.expect("a batch").0.len();
        if round >= WARMUP {
            nanos.push((figures.0 + ns) / u64::try_from(FRAMES).expect("few"));
            allocations += figures.1 + allocs;
        }
    }
    (nanos, allocations, body)
}

/// Defines the index `edge.time` and the `f64` channels `edge.c<i>` for each `i`
/// below `channels`, and gives each name.
fn define(hub: &hub::Hub, channels: usize) -> Vec<Name> {
    let time = channel::Key::from_u128(1);
    let index = Kind::Index {
        error: None,
        control: None,
    };
    let mut definitions = vec![(
        name("edge.time"),
        Definition::Channel(Channel {
            key: time,
            kind: index,
        }),
    )];
    for at in 0..channels {
        let data_type = DataType::Sample(Type::Scalar(Scalar::F64));
        let data = Data::new(time, None, data_type, None).expect("no unit");
        let key = channel::Key::from_u128(u128::try_from(at).expect("few") + 2);
        let channel = Channel {
            key,
            kind: Kind::Data(data),
        };
        definitions.push((name(&format!("edge.c{at}")), Definition::Channel(channel)));
    }
    hub.set_definitions(
        definitions
            .iter()
            .map(|(name, definition)| (name, definition)),
    );
    definitions.into_iter().map(|(name, _)| name).collect()
}

/// Writes one frame of each channel of `writer`, with `samples` sample times from
/// `first`, each nanosecond after the one before it.
fn write(writer: &mut Writer, first: i64, samples: usize) {
    let entries = writer.set().entries();
    let index = entries
        .iter()
        .position(|entry| entry.key == channel::Key::from_u128(1))
        .expect("the index");
    let group = entries[index].group;
    let series: Vec<_> = (0..entries.len()).map(|at| (at, samples * 8)).collect();
    let mut draft = writer.draft(Form::Raw, &series).expect("a frame");
    for at in 0..entries.len() {
        let series = draft.series_mut(at).expect("a series");
        let bytes = (0..samples).flat_map(|sample| {
            let sample = i64::try_from(sample).expect("few");
            if at == index {
                (first + sample).to_le_bytes()
            } else {
                (1.5 * f64::from(u8::try_from(sample % 200).expect("fits")))
                    .to_le_bytes()
            }
        });
        for (to, from) in series.iter_mut().zip(bytes) {
            *to = from;
        }
    }
    draft.set_count(group, u32::try_from(samples).expect("few"));
    let written = writer.write(Label::Path(Path::Live), draft);
    let applied = written.is_ok_and(|outcomes| {
        (outcomes.iter())
            .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }))
    });
    assert!(applied, "the frame applies");
}

/// The result of `f`, and its ns and allocations.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn timed<T>(f: impl FnOnce() -> T) -> (T, (u64, u64)) {
    let ((value, span), allocations) = ALLOCATOR.count(|| {
        let start = Instant::now();
        let value = f();
        (value, Instant::now().duration_since(start))
    });
    let nanos = u64::try_from(span.as_nanos()).expect("a call takes under 2^64 ns");
    (value, (nanos, allocations))
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// `value` as a float, exact below 2^53.
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "a printed figure loses no digit it shows"
)]
fn per(value: impl TryInto<u64, Error: std::fmt::Debug>) -> f64 {
    value.try_into().expect("fits 64 bits") as f64
}
