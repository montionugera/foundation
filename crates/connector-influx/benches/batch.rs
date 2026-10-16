//! The cost of the path from a reader to the body of an InfluxDB request, on one shard
//! of a sim node. Run with `cargo bench -p connector-influx --bench batch`.
//!
//! For each shape, a writer writes frames of an index and `channels` channels of the
//! shape's types in turn (`f64` only, `MIXED`, `STRING`, `UUID`, `BYTES`, or `TEXT`),
//! with `samples` sample times each, and a complete reader reads them. Each frame holds
//! `present` of the channels, a different `present` in turn, so the frames hold each
//! channel once before they hold one again. The batch writes to a bucket, so a `u64`
//! channel is a `u` field. Each round adds `FRAMES` frames to one batch with
//! `Batch::add`, then takes its body with `Batch::take`, as the send loop does for one
//! request. Only these two calls are timed. The figures are per frame: the ns of the
//! warm-up rounds, in which the batch first sees each channel; the ns of each later
//! round, at percentiles over the rounds; the ns for each sample of a channel in the
//! frame at p50; and the allocations. It checks that `Batch::add` makes no allocation
//! after the warm-up rounds.
//!
//! Then, for each count in `NEW`, it times `Batch::add` of one frame of that many
//! channels, one sample time each, on a new batch, so each channel is new to it.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::time::Instant;

use connector::kind::Table;
use connector::testing;
use connector_influx::Target;
use connector_influx::bench::Batch;
use connector_influx::line::Measurement;
use hub::Hub;
use hub::reader::{Mode, Reader};
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
/// Few enough that each frame of each shape fits the ring of the sim hub, which no ack
/// frees.
const ROUNDS: usize = 20;
/// Each `(channels, samples, present, types)`.
const SHAPES: [(usize, usize, usize, &[Type]); 16] = [
    (1, 1, 1, F64),
    (8, 1, 8, F64),
    (64, 1, 64, F64),
    (1, 100, 1, F64),
    (8, 100, 8, F64),
    (64, 10, 64, F64),
    (8, 1, 4, F64),
    (64, 10, 32, F64),
    (4096, 1, 1, F64),
    (8, 1, 8, MIXED),
    (64, 10, 64, MIXED),
    (8, 1, 8, STRING),
    (8, 1, 8, UUID),
    (8, 1, 8, BYTES),
    (8, 1, 8, TEXT),
    (64, 10, 64, TEXT),
];
const F64: &[Type] = &[Type::Scalar(Scalar::F64)];
/// A type of each kind of field: boolean, `i`, `u`, and float.
const MIXED: &[Type] = &[
    Type::Scalar(Scalar::Bool),
    Type::Scalar(Scalar::I32),
    Type::Scalar(Scalar::I64),
    Type::Scalar(Scalar::U32),
    Type::Scalar(Scalar::U64),
    Type::Scalar(Scalar::F32),
    Type::Scalar(Scalar::F64),
    Type::Scalar(Scalar::Stamp),
];
const STRING: &[Type] = &[Type::String];
const UUID: &[Type] = &[Type::Scalar(Scalar::Uuid)];
const BYTES: &[Type] = &[Type::Bytes];
/// Each type of a string field.
const TEXT: &[Type] = &[Type::String, Type::Scalar(Scalar::Uuid), Type::Bytes];
/// Each count of channels of a frame that a new batch adds.
const NEW: [usize; 4] = [1, 64, 256, 1024];

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    println!("ns and allocations per frame over {ROUNDS} rounds of {FRAMES} frames");
    println!(
        "{:>8} {:>7} {:>7} {:>6} {:>9} {:>9} {:>9} {:>9} {:>11} {:>12} {:>11}",
        "channels",
        "samples",
        "present",
        "types",
        "first",
        "p10",
        "p50",
        "p90",
        "ns/sample",
        "allocs/frame",
        "body bytes"
    );
    for (channels, samples, present, types) in SHAPES {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let shape = (channels, samples, present);
        let run = move |node, tasks| bench(node, tasks, shape, types);
        let (first, mut nanos, allocations, body) =
            sim.run_on(&node, run).expect("the run ends");
        nanos.sort_unstable();
        let at = |percent: usize| nanos[nanos.len() * percent / 100];
        let frames = ROUNDS * FRAMES;
        println!(
            "{channels:>8} {samples:>7} {present:>7} {:>6} {first:>9} {:>9} {:>9} \
             {:>9} {:>11.2} {:>12.2} {body:>11}",
            match types {
                F64 => "f64",
                MIXED => "mixed",
                STRING => "string",
                UUID => "uuid",
                BYTES => "bytes",
                _ => "text",
            },
            at(10),
            at(50),
            at(90),
            per(at(50)) / per(present * samples),
            per(allocations) / per(frames),
        );
    }
    println!(
        "\nns and allocations of the first frame on a new batch, over {ROUNDS} batches"
    );
    println!(
        "{:>8} {:>9} {:>9} {:>9} {:>11} {:>12}",
        "channels", "p10", "p50", "p90", "ns/channel", "allocs/frame"
    );
    for channels in NEW {
        let mut sim = sim::Sim::new(sim::Config::default());
        let node = sim.node(sim::node::Config::default());
        let run = move |node, tasks| first(node, tasks, channels);
        let (mut nanos, allocations) = sim.run_on(&node, run).expect("the run ends");
        nanos.sort_unstable();
        let at = |percent: usize| nanos[nanos.len() * percent / 100];
        println!(
            "{channels:>8} {:>9} {:>9} {:>9} {:>11.2} {:>12.2}",
            at(10),
            at(50),
            at(90),
            per(at(50)) / per(channels),
            per(allocations) / per(ROUNDS),
        );
    }
}

/// The ns of `Batch::add` of one frame of `channels` channels on each of `ROUNDS`
/// new batches, and the allocations of all of them.
async fn first(
    node: sim::node::Node,
    tasks: env::tasks::Tasks,
    channels: usize,
) -> (Vec<u64>, u64) {
    let (_hub, mut writer, mut reader) = open(&node, tasks, channels, F64).await;
    let stamp = writer.now().nanos();
    write(&mut writer, stamp, 1, (1, 0));
    node.clock().sleep(Span::SECOND).await;
    let received = reader.next().await.expect("a frame");
    let (mut nanos, mut allocations) = (Vec::new(), 0);
    for _ in 0..ROUNDS {
        let mut batch =
            Batch::new(telemetry(), &bucket(), &"influx".parse().expect("a name"));
        let (added, (ns, allocs)) = timed(|| batch.add(&received));
        added.expect("each channel holds a type that a bucket stores");
        nanos.push(ns);
        allocations += allocs;
    }
    (nanos, allocations)
}

/// The ns per frame of the warm-up rounds, the ns per frame of each timed round, the
/// allocations of all timed rounds, and the bytes of the last body.
async fn bench(
    node: sim::node::Node,
    tasks: env::tasks::Tasks,
    (channels, samples, present): (usize, usize, usize),
    types: &[Type],
) -> (u64, Vec<u64>, u64, usize) {
    let (_hub, mut writer, mut reader) = open(&node, tasks, channels, types).await;
    let mut batch =
        Batch::new(telemetry(), &bucket(), &"influx".parse().expect("a name"));
    let (mut nanos, mut allocations, mut body) = (Vec::new(), 0, 0);
    let groups = channels / present;
    let warmup = groups.div_ceil(FRAMES) + 1;
    let mut first = 0;
    let mut stamp = writer.now().nanos();
    for round in 0..warmup + ROUNDS {
        for frame in 0..FRAMES {
            let group = (round * FRAMES + frame) % groups;
            write(&mut writer, stamp, samples, (groups, group));
            stamp += i64::try_from(samples).expect("few");
        }
        node.clock().sleep(Span::SECOND).await;
        let mut figures = (0, 0);
        for _ in 0..FRAMES {
            let received = reader.next().await.expect("a frame");
            let (added, (ns, allocs)) = timed(|| batch.add(&received));
            added.expect("each channel holds a type that a bucket stores");
            figures = (figures.0 + ns, figures.1 + allocs);
            let position = received.position();
            reader.ack(position);
        }
        let (taken, (ns, allocs)) = timed(|| batch.take());
        body = taken.expect("a batch").body.len();
        if round < warmup {
            first += figures.0 + ns;
        } else {
            assert_eq!(
                figures.1, 0,
                "{channels} x {samples}: add allocates nothing"
            );
            nanos.push((figures.0 + ns) / u64::try_from(FRAMES).expect("few"));
            allocations += figures.1 + allocs;
        }
    }
    let frames = u64::try_from(warmup * FRAMES).expect("few");
    (first / frames, nanos, allocations, body)
}

/// A hub of the index `edge.time` and `channels` channels of `types` in turn, a writer
/// of each channel, and a complete reader of them.
async fn open(
    node: &sim::node::Node,
    tasks: env::tasks::Tasks,
    channels: usize,
    types: &[Type],
) -> (Hub, Writer, Reader) {
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
    let names = define(&hub, channels, types);
    let config = hub::writer::Config {
        subject: name("bench.writer"),
        authority: Authority::ABSOLUTE,
        lease: None,
        channels: names,
    };
    let writer = hub.writer(config).await.expect("the writer opens");
    let config = hub::reader::Config {
        select: Selector::new(["edge.**"]).expect("a selector"),
        mode: Mode::Complete,
        subject: name("bench.reader"),
        name: None,
        hold: Span::ZERO,
    };
    let reader = hub.reader(config).await.expect("the reader opens");
    (hub, writer, reader)
}

/// Defines the index `edge.time` and the channels `edge.c<i>` for each `i` below
/// `channels`, of `types` in turn, and gives each name.
fn define(hub: &hub::Hub, channels: usize, types: &[Type]) -> Vec<Name> {
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
        let data_type = DataType::Sample(types[at % types.len()]);
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
/// `first`, each nanosecond after the one before it. Of `(groups, turn)`, it writes
/// only the data channels whose entry is `turn` modulo `groups`.
fn write(
    writer: &mut Writer,
    first: i64,
    samples: usize,
    (groups, turn): (usize, usize),
) {
    let entries = writer.set().entries();
    let index = entries
        .iter()
        .position(|entry| entry.key == channel::Key::from_u128(1))
        .expect("the index");
    let group = entries[index].group;
    let series: Vec<_> = (0..entries.len())
        .filter(|&at| at == index || at % groups == turn)
        .map(|at| {
            (
                at,
                raw(entries[at].data_type, at, at == index, first, samples),
            )
        })
        .collect();
    let lens: Vec<_> = series.iter().map(|(at, raw)| (*at, raw.len())).collect();
    let mut draft = writer.draft(Form::Raw, &lens).expect("a frame");
    for (at, raw) in &series {
        draft
            .series_mut(*at)
            .expect("a series")
            .copy_from_slice(raw);
    }
    draft.set_count(group, u32::try_from(samples).expect("few"));
    let written = writer.write(Label::Path(Path::Live), draft);
    let applied = written.is_ok_and(|outcomes| {
        (outcomes.iter())
            .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }))
    });
    assert!(applied, "the frame applies");
}

/// The raw series of `samples` samples of `data_type` from `first`, of the index when
/// `index`, each nanosecond after the one before it. A variable sample is a text with
/// two quotes, which a series of one sample repeats 1 to 64 times by its entry `at`, so
/// it is 9 to 576 bytes, 11 to 704 as written, and the same in each frame. A longer
/// series has the text once, so the ring of the sim hub holds it.
fn raw(data_type: Type, at: usize, index: bool, first: i64, samples: usize) -> Vec<u8> {
    let small = |sample: usize| u8::try_from(sample % 200).expect("fits");
    if let Some(form) = codec::Variable::of(data_type) {
        let repeats = if samples == 1 { at * 9 % 64 + 1 } else { 1 };
        let texts: Vec<_> = (0..samples)
            .map(|sample| {
                let text = format!("value \"{}\"", small(sample));
                text.repeat(repeats).into_bytes()
            })
            .collect();
        let mut bytes = vec![0; form.len(&texts).expect("fits")];
        form.write(&texts, &mut bytes);
        return bytes;
    }
    let width = data_type.width().expect("a fixed-width type");
    (0..samples)
        .flat_map(|sample| {
            let small = small(sample);
            let bytes = match data_type {
                _ if index => i128::from(first + i64::try_from(sample).expect("few"))
                    .to_le_bytes(),
                Type::Scalar(Scalar::F64) => {
                    u128::from((1.5 * f64::from(small)).to_bits()).to_le_bytes()
                }
                Type::Scalar(Scalar::F32) => {
                    u128::from((1.5 * f32::from(small)).to_bits()).to_le_bytes()
                }
                Type::Scalar(Scalar::Bool) => u128::from(small % 2).to_le_bytes(),
                _ => u128::from(small).to_le_bytes(),
            };
            bytes.into_iter().take(width)
        })
        .collect()
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

fn telemetry() -> Measurement {
    Measurement::new("telemetry", &[]).expect("a valid measurement")
}

/// A bucket, so a `u64` channel is a `u` field.
fn bucket() -> Target {
    Target::Bucket {
        name: "plant".into(),
        org: "o".into(),
    }
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
