//! The `opcua` kind: reads OPC UA nodes at a fixed rate, and writes at most one frame
//! per tick.

use std::{fmt, future, mem};

use connector::kind::{self, Channels, Context, Error};
use connector::stamp::Stamps;
use connector::{cancel, pace, status};
use document::Document;
use document::diagnostic::{Code, Diagnostic};
use hub::home::{Outcome, Refusal, order};
use hub::writer::Writer;
use types::authority::Authority;
use types::frame::{self, Draft, Form, Label, Path, key_set};
use types::sample::{self, Scalar, Type};
#[cfg(feature = "sim")]
use types::time::Span;
use types::time::Stamp;

use crate::client::{Client, Large};
use crate::config::{self, Config, Read};
use crate::event;
use crate::ffi::Status;
use crate::node::Node;
use crate::value::{self, Cause, Fault, SCALARS, Value};

#[cfg(test)]
#[cfg(feature = "sim")]
mod tests;

const SAMPLE_TYPE: Code = Code::new("opcua.sample-type");
const LARGE_READ: Code = Code::new("opcua.large-read");

/// The counts of the status of each connector: the samples that the home applied,
/// the ticks that the timer skipped, the samples that a frame lost, and the values
/// that a frame leaves out as Bad, as Uncertain, and as of another type.
const COUNTS: [&str; 6] = ["in", "missed", "lost", "bad", "uncertain", "mistyped"];

/// The `opcua` kind: reads the Value of OPC UA nodes at a fixed rate, in Read requests
/// that each fit one message of the server, and writes at most one frame per tick.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Kind;

impl kind::Kind for Kind {
    type Config = Config;

    /// Reads `address`, `rate`, and one or more `read` blocks, each with the channel
    /// as its label and a `node_id`, and no other key or block.
    fn parse(&self, config: &Document) -> Result<Config, Vec<Diagnostic>> {
        config::parse(config)
    }

    /// Gives the channel of each `read` block as a write, in order, and the counts
    /// `in`, `missed`, `lost`, `bad`, `uncertain`, and `mistyped`.
    fn check(&self, config: &Config) -> Result<Channels, Vec<Diagnostic>> {
        let counts = COUNTS.map(|count| count.parse().expect("a count is a name"));
        Ok(Channels {
            reader: None,
            writes: config
                .reads
                .iter()
                .map(|read| read.channel.clone())
                .collect(),
            counts: counts.into(),
        })
    }

    /// Finds nothing: the kind does not browse a server.
    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        future::ready(Ok(Vec::new()))
    }

    /// Connects to the server, and reads each node once per tick of the rate until
    /// the cancel. Each tick writes a frame of the values that are Good and of the
    /// sample type of their channel (Boolean for `bool`, `SByte` to `UInt64` for `i8`
    /// to `u64`, Float for `f32`, and Double for `f64`), stamped at the midpoint of the
    /// read. It leaves out each other value: an Uncertain one adds 1 to the count
    /// `uncertain`, another one that is not Good adds 1 to `bad`, and one of another
    /// type adds 1 to `mistyped`. The note names the first node of another type of
    /// the latest tick that has one, and a tick with none clears it. A tick that
    /// leaves out each value writes no frame. When 10 s pass with no read, the run
    /// reads the state of the server to check the connection. After a cancel or an
    /// error, the run closes its session: it sends `CloseSession`, waits at most 1 s
    /// for the answer, then closes the connection. So a run ends within 1 s of a
    /// cancel.
    ///
    /// # Errors
    ///
    /// The error of [`Context::writer`] and of each failed write. [`Error::Config`]
    /// with `opcua.sample-type` for each channel whose sample type is not `bool`, `i8`
    /// to `u64`, `f32`, or `f64`, and with `opcua.large-read` when the Read of one node
    /// alone is past one message of the server. [`Error::Device`] when the connect, a
    /// read, or one check fails: a check fails on a status that is not Good or on no
    /// answer in 5 s.
    fn run(&self, ctx: Context<Config>) -> impl Future<Output = Result<(), Error>> {
        run(ctx)
    }
}

async fn run(ctx: Context<Config>) -> Result<(), Error> {
    let mut frames = Frames::open(&ctx).await?;
    let config = ctx.config();
    let mesh = frames.out.writer.time();
    let time = event::Time {
        clock: ctx.clock().clone(),
        wall: Box::new(move || mesh.now()),
    };
    let rng = &mut ctx.rng();
    let net = ctx.net().clone();
    let cancelled = ctx.cancel().wait();
    match Client::connect(time, net, rng, &config.address, cancelled).await {
        Ok(None) => Ok(()),
        Err(status) => Err(device(Failure::Connect {
            address: config.address.to_string_lossy().into_owned(),
            status,
        })),
        Ok(Some(mut client)) => {
            let result = read(&ctx, &mut client, &mut frames).await;
            client.close().await;
            result
        }
    }
}

/// Reads each node once per tick until the cancel, and writes the frame of each tick.
async fn read(
    ctx: &Context<Config>,
    client: &mut Client,
    frames: &mut Frames<'_>,
) -> Result<(), Error> {
    let reads = &ctx.config().reads;
    let nodes: Vec<Node> = reads.iter().map(|read| read.node.clone()).collect();
    let nodes = client
        .nodes(&nodes)
        .map_err(|large| large_read(reads, large))?;
    let missed = ctx.count("missed");
    let mut timer = pace::Timer::new(ctx.clock(), ctx.config().rate);
    loop {
        let idle = client.idle(timer.tick(ctx.cancel())).await;
        let Some(tick) = idle.map_err(|status| device(Failure::Check(status)))? else {
            break;
        };
        missed.add(tick.missed);
        let Some((read, stamp)) =
            ctx.cancel().race(frames.request(client.read(&nodes))).await
        else {
            break;
        };
        let values = read.map_err(|status| device(Failure::Read(status)))?;
        frames.write(values, stamp)?;
    }
    Ok(())
}

/// Writes `count` frames of the reads of `ctx` with no server, each value 1 in the
/// sample type of its channel (`true` for `bool`) but the first `bad(i)` of write `i`
/// (from 0), which are `BadNotFound` and left out, and each frame 1 ms of its clock
/// after the last, and gives what `measure` gives for the check of the values and the
/// write of each frame. `measure` calls the work that it gets once. For benchmarks.
///
/// # Errors
///
/// As [`Kind::run`](kind::Kind::run), with no connect or read.
///
/// # Panics
///
/// When the home refuses a frame for a cause that a run never gets: `Backwards` after
/// the stamp that a refusal gives, `Expired`, or `Codec`.
#[cfg(feature = "sim")]
pub async fn writes(
    ctx: &Context<Config>,
    count: usize,
    bad: impl Fn(usize) -> usize,
    mut measure: impl FnMut(&mut dyn FnMut()) -> u64,
) -> Result<Vec<u64>, Error> {
    let mut frames = Frames::open(ctx).await?;
    let mut measures = Vec::with_capacity(count);
    for write in 0..count {
        let bad = bad(write);
        let types = frames.layout.types.iter().enumerate();
        let values: Vec<_> = types
            .map(|(at, &scalar)| {
                if at < bad {
                    Err(Fault::status(Status::BAD_NOT_FOUND))
                } else {
                    Ok(Value::one(scalar))
                }
            })
            .collect();
        ctx.clock().sleep(Span::from_nanos(1_000_000)).await;
        let ((), stamp) = frames.request(future::ready(())).await;
        let mut written = Ok(());
        measures.push(measure(&mut || {
            written = frames.write(&values, stamp);
        }));
        written?;
    }
    Ok(measures)
}

/// The frames of the ticks of a run: one sample in each series of the key set of its
/// writer that a tick keeps, the stamp in each index and a value in each other.
struct Frames<'a> {
    /// The context of the run, whose reads give the values of each frame, in order.
    ctx: &'a Context<Config>,
    layout: Layout,
    out: Out,
    stamps: Stamps,
    left: Left,
    kept: Kept,
    /// The series of the groups that a frame writes again.
    again: Vec<(usize, usize)>,
    /// The groups that the home refused as `Backwards`, in order.
    backwards: Vec<u32>,
}

/// The series of the frame of a tick that keeps each value, and what fills each.
struct Layout {
    /// The sample type of the channel of each read, in order.
    types: Vec<Scalar>,
    /// The sample of each entry of the key set.
    fills: Vec<Fill>,
    series: Vec<(usize, usize)>,
    samples: Vec<u64>,
    groups: Vec<u32>,
}

impl Layout {
    fn shape(&self) -> Shape<'_> {
        Shape {
            series: &self.series,
            samples: &self.samples,
            groups: &self.groups,
        }
    }
}

/// The series of one frame.
#[derive(Clone, Copy)]
struct Shape<'a> {
    /// Each entry of the key set in the frame and the bytes of its series.
    series: &'a [(usize, usize)],
    /// The count of data samples of each group in the frame, by its number.
    samples: &'a [u64],
    /// The group of each index in the frame, in the order of the key set.
    groups: &'a [u32],
}

/// The counts of the values that a frame leaves out.
struct Left {
    bad: status::Count,
    uncertain: status::Count,
    mistyped: status::Count,
    /// The read and the type that the note names, or `None` when it shows none.
    noted: Option<(usize, value::Type)>,
}

/// The series of the frame of the last tick that left out a value. Each `Vec` has the
/// capacity of the layout from the start, so no tick allocates.
struct Kept {
    /// Whether the frame keeps the value of each read, in order. Empty after a tick
    /// that keeps each value.
    reads: Vec<bool>,
    /// `reads` of the tick before.
    last: Vec<bool>,
    series: Vec<(usize, usize)>,
    samples: Vec<u64>,
    groups: Vec<u32>,
}

/// The writer of the frames, and the counts of their samples.
struct Out {
    writer: Writer,
    applied: status::Count,
    lost: status::Count,
}

impl<'a> Frames<'a> {
    /// Opens the writer of the channels of the reads of `ctx`, and gives its frames.
    ///
    /// # Errors
    ///
    /// The error of [`Context::writer`]. [`Error::Config`] with `opcua.sample-type`
    /// for each channel whose sample type is not one of [`SCALARS`].
    async fn open(ctx: &'a Context<Config>) -> Result<Self, Error> {
        let reads = &ctx.config().reads;
        let channels = reads.iter().map(|read| read.channel.clone()).collect();
        let writer = ctx.writer(channels, Authority::ABSOLUTE, None).await?;
        let set = writer.set();
        let entries = set.entries();
        let (mut types, mut wrong) = (Vec::with_capacity(reads.len()), Vec::new());
        for (read, &entry) in reads.iter().zip(writer.entries()) {
            let data_type = entries[entry].data_type;
            match data_type {
                Type::Scalar(scalar) if SCALARS.contains(&scalar) => types.push(scalar),
                _ => wrong.push(sample_type(read, data_type)),
            }
        }
        if !wrong.is_empty() {
            return Err(Error::Config(wrong));
        }
        let mut reads_of = vec![None; entries.len()];
        for (read, &entry) in writer.entries().iter().enumerate() {
            reads_of[entry] = Some(read);
        }
        let mut samples = vec![0; set.groups().len()];
        let fills: Vec<_> = (entries.iter().zip(reads_of))
            .map(|(&key_set::Entry { group, .. }, read)| {
                if read.is_some() {
                    samples[to_usize(group)] += 1;
                }
                Fill { group, read }
            })
            .collect();
        let groups = (fills.iter())
            .filter(|fill| fill.read.is_none())
            .map(|fill| fill.group)
            .collect();
        let width = |entry: &key_set::Entry| {
            (entry.data_type.width())
                .expect("invariant: an index or a read is a scalar")
        };
        let series: Vec<_> = entries.iter().map(width).enumerate().collect();
        let layout = Layout {
            types,
            fills,
            series,
            samples,
            groups,
        };
        Ok(Self {
            ctx,
            again: Vec::with_capacity(layout.series.len()),
            backwards: Vec::with_capacity(layout.samples.len()),
            out: Out {
                applied: ctx.count("in"),
                lost: ctx.count("lost"),
                writer,
            },
            stamps: Stamps::default(),
            left: Left {
                bad: ctx.count("bad"),
                uncertain: ctx.count("uncertain"),
                mistyped: ctx.count("mistyped"),
                noted: None,
            },
            kept: Kept::new(&layout),
            layout,
        })
    }

    /// Runs `request`, and gives its output with the stamp of its frame.
    async fn request<T>(&mut self, request: impl Future<Output = T>) -> (T, Stamp) {
        self.stamps.request(&self.out.writer, request).await
    }

    /// Writes one frame of `values`, the values of the reads in order, at `stamp`,
    /// with each value that is Good and of the sample type of its channel. It counts
    /// each other value in [`Left`], and writes no frame when it keeps none. A group
    /// that the home refuses as `Backwards` is written again once, after the stamp
    /// that the refusal gives.
    ///
    /// # Errors
    ///
    /// As [`Out::send`].
    ///
    /// # Panics
    ///
    /// When the home refuses a group as `Backwards` again.
    fn write(
        &mut self,
        values: &[Result<Value, Fault>],
        stamp: Stamp,
    ) -> Result<(), Error> {
        let Self {
            ctx,
            layout,
            out,
            stamps,
            left,
            kept,
            again,
            backwards,
        } = self;
        let Some((shape, values)) = kept.sort(left, ctx, layout, values) else {
            return Ok(());
        };
        let fills = &layout.fills;
        let first = out.send(fills, shape, values, stamp, backwards)?;
        let Some(before) = first else {
            return Ok(());
        };
        let stamp = stamps.after(before);
        let refused =
            |&&(entry, _): &&(usize, usize)| backwards.contains(&fills[entry].group);
        again.clear();
        again.extend(shape.series.iter().filter(refused));
        let shape = Shape {
            series: again,
            groups: backwards,
            ..shape
        };
        let last = out.send(fills, shape, values, stamp, &mut Vec::new())?;
        if let Some(again) = last {
            panic!(
                "invariant: a frame stamped after {before}, the last stamp of its \
                 index, is not backwards, but the home gives {again} as last"
            );
        }
        Ok(())
    }
}

impl Kept {
    /// The selection of no tick yet, with the capacity of each frame of `layout`.
    fn new(layout: &Layout) -> Self {
        let reads = layout.types.len();
        Self {
            reads: Vec::with_capacity(reads),
            last: Vec::with_capacity(reads),
            series: Vec::with_capacity(layout.series.len()),
            samples: Vec::with_capacity(layout.samples.len()),
            groups: Vec::with_capacity(layout.groups.len()),
        }
    }

    /// Counts in `left` each value of `values` that a frame leaves out: one that is
    /// not Good, or one whose type is not the sample type of its channel in `layout`.
    /// The note of `ctx` names the first node of another type, and a tick with none
    /// clears it. Gives the series of the frame that keeps the other values, with
    /// `values`, or `None` when it keeps no value.
    fn sort<'s, 'v>(
        &'s mut self,
        left: &mut Left,
        ctx: &Context<Config>,
        layout: &'s Layout,
        values: &'v [Result<Value, Fault>],
    ) -> Option<(Shape<'s>, Values<'v>)> {
        let mut pairs = values.iter().zip(&layout.types);
        if pairs.all(|(value, &sample)| Why::of(value, sample).is_none()) {
            left.note(ctx, None);
            self.reads.clear();
            return Some((layout.shape(), Values(values)));
        }
        mem::swap(&mut self.reads, &mut self.last);
        self.reads.clear();
        let (mut bad, mut uncertain, mut mistyped) = (0, 0, 0);
        let mut first = None;
        for (at, (value, &sample)) in values.iter().zip(&layout.types).enumerate() {
            let why = Why::of(value, sample);
            match why {
                None => {}
                Some(Why::Bad) => bad += 1,
                Some(Why::Uncertain) => uncertain += 1,
                Some(Why::Mistyped(value)) => {
                    mistyped += 1;
                    first = first.or(Some((at, value, sample)));
                }
            }
            self.reads.push(why.is_none());
        }
        left.add(bad, uncertain, mistyped);
        left.note(ctx, first);
        if self.reads != self.last {
            self.keep(layout);
        }
        if self.groups.is_empty() {
            return None;
        }
        let shape = Shape {
            series: &self.series,
            samples: &self.samples,
            groups: &self.groups,
        };
        Some((shape, Values(values)))
    }

    /// Puts in `self` the series of `layout` that the frame keeps: the series of each
    /// read that `reads` keeps, and the index of each group that keeps one of them.
    fn keep(&mut self, layout: &Layout) {
        let Self {
            reads,
            series,
            samples,
            groups,
            ..
        } = self;
        samples.clear();
        samples.resize(layout.samples.len(), 0);
        for fill in &layout.fills {
            if fill.read.is_some_and(|read| reads[read]) {
                samples[to_usize(fill.group)] += 1;
            }
        }
        let held = |&&(entry, _): &&(usize, usize)| {
            let Fill { group, read } = layout.fills[entry];
            read.map_or(samples[to_usize(group)] > 0, |read| reads[read])
        };
        series.clear();
        series.extend(layout.series.iter().filter(held));
        groups.clear();
        let present = |&&group: &&u32| samples[to_usize(group)] > 0;
        groups.extend(layout.groups.iter().filter(present));
    }
}

impl Left {
    /// Adds the counts of the values that one frame leaves out.
    fn add(&self, bad: u64, uncertain: u64, mistyped: u64) {
        self.bad.add(bad);
        self.uncertain.add(uncertain);
        self.mistyped.add(mistyped);
    }

    /// Sets the note of `ctx` for `mistyped`, when it is not the note of the tick
    /// before: the read at a value of another type, its type, and the sample type of
    /// its channel. `None` clears the note.
    #[inline]
    fn note(
        &mut self,
        ctx: &Context<Config>,
        mistyped: Option<(usize, value::Type, Scalar)>,
    ) {
        if mistyped.map(|(at, value, _)| (at, value)) != self.noted {
            self.show(ctx, mistyped);
        }
    }

    #[cold]
    fn show(
        &mut self,
        ctx: &Context<Config>,
        mistyped: Option<(usize, value::Type, Scalar)>,
    ) {
        self.noted = mistyped.map(|(at, value, _)| (at, value));
        let Some((at, value, sample)) = mistyped else {
            ctx.note(&"");
            return;
        };
        let read = &ctx.config().reads[at];
        ctx.note(&format_args!(
            "leaves out the values of the node `{}` of the channel `{}`: they are of \
             type {value}, and the channel has samples of type {}",
            read.node,
            read.channel,
            Type::Scalar(sample),
        ));
    }
}

/// Why a frame leaves out a value.
#[derive(Debug, PartialEq)]
enum Why {
    Bad,
    Uncertain,
    /// The value is Good, of this type, which is not the type of its channel.
    Mistyped(value::Type),
}

impl Why {
    /// Why a frame leaves out `value`, whose channel has samples of type `sample`,
    /// or `None` when it keeps it.
    fn of(value: &Result<Value, Fault>, sample: Scalar) -> Option<Self> {
        match value {
            Ok(value) if value.scalar() == sample => None,
            Ok(value) => Some(Self::Mistyped(value.type_of())),
            Err(fault) => match fault.cause() {
                Cause::Type(opc_type) => Some(Self::Mistyped(opc_type)),
                Cause::Status(status) if status.uncertain() => Some(Self::Uncertain),
                Cause::Status(_) => Some(Self::Bad),
            },
        }
    }
}

impl Out {
    /// Writes one frame of `shape` with `values` at `stamp`, each series filled as
    /// `fills` gives, and counts its samples. A frame for which the pool has no block
    /// now loses its samples.
    ///
    /// Returns the latest stamp that the home gives for a group that it refuses as
    /// `Backwards`, and puts those groups in `backwards`.
    ///
    /// # Errors
    ///
    /// The error of a write, and [`Error::Config`] with `connector.large-frame` when
    /// the frame is larger than the largest block of the pool.
    ///
    /// # Panics
    ///
    /// When the home refuses a group as `Expired` or `Codec`, which a writer with no
    /// lease and a raw frame never gets.
    fn send(
        &mut self,
        fills: &[Fill],
        shape: Shape<'_>,
        values: Values<'_>,
        stamp: Stamp,
        backwards: &mut Vec<u32>,
    ) -> Result<Option<Stamp>, Error> {
        let Shape {
            series,
            samples,
            groups,
        } = shape;
        let mut draft = match self.writer.draft(Form::Raw, series) {
            Ok(draft) => draft,
            Err(frame::Error::Pool(
                block::Error::Exhausted { .. } | block::Error::Refused { .. },
            )) => {
                let lost = groups.iter().map(|&group| samples[to_usize(group)]).sum();
                self.lost.add(lost);
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        fill(&mut draft, fills, values, stamp, groups);
        let outcomes = self.writer.write(Label::Path(Path::Live), draft)?;
        backwards.clear();
        let mut latest = None;
        let (mut wrote, mut gone) = (0, 0);
        for (&group, outcome) in groups.iter().zip(outcomes) {
            let n = samples[to_usize(group)];
            match outcome {
                Outcome::Applied { .. } => wrote += n,
                Outcome::Refused {
                    refusal: Refusal::Order(order::Error::Backwards { before, .. }),
                    ..
                } => {
                    backwards.push(group);
                    latest = latest.max(Some(*before));
                }
                Outcome::Lost { .. }
                | Outcome::Refused {
                    refusal: Refusal::Waiting | Refusal::Reserved | Refusal::Order(_),
                    ..
                } => gone += n,
                Outcome::Refused {
                    refusal: refusal @ (Refusal::Expired | Refusal::Codec { .. }),
                    ..
                } => panic!(
                    "invariant: the home refuses no frame with no lease in raw form, \
                     but gives {refusal}"
                ),
            }
        }
        self.applied.add(wrote);
        self.lost.add(gone);
        Ok(latest)
    }
}

/// The sample of one entry of the key set in each frame.
struct Fill {
    group: u32,
    /// The position of the read of the entry in the config, or `None` for an index,
    /// whose sample is the stamp.
    read: Option<usize>,
}

/// Fills each series of `draft` with one sample: `stamp` in each index of `groups`, and
/// the value of its read in each other.
fn fill(
    draft: &mut Draft,
    fills: &[Fill],
    values: Values<'_>,
    stamp: Stamp,
    groups: &[u32],
) {
    for (entry, bytes) in draft.iter_mut() {
        match fills[entry].read {
            None => sample::Value::Stamp(stamp).write(bytes),
            Some(read) => sample::Value::from(values.get(read)).write(bytes),
        }
    }
    for &group in groups {
        draft.set_count(group, 1);
    }
}

/// The values of one read. Each value that a frame holds is Good and of the sample
/// type of its channel.
#[derive(Clone, Copy)]
struct Values<'a>(&'a [Result<Value, Fault>]);

impl Values<'_> {
    /// The value of the read at `at`.
    fn get(self, at: usize) -> Value {
        match self.0[at] {
            Ok(value) => value,
            other => broken(at, other),
        }
    }
}

/// Panics with the read at `at` and its value. Cold and apart from `Values::get`, so
/// the loop of a send stores nothing per sample for the message.
#[cold]
#[inline(never)]
fn broken(at: usize, value: Result<Value, Fault>) -> ! {
    panic!("invariant: read {at} is {value:?}, which `Kept::sort` leaves out")
}

/// The diagnostic of the channel of `read`, whose sample type `data_type` is not one
/// of [`SCALARS`].
fn sample_type(read: &Read, data_type: Type) -> Diagnostic {
    let names = SCALARS.map(|scalar| Type::Scalar(scalar).to_string());
    let (last, others) = names.split_last().expect("invariant: SCALARS has types");
    Diagnostic::new(
        SAMPLE_TYPE,
        None,
        format!(
            "the channel `{}` has samples of type {data_type}, and the opcua kind \
             reads only {}, and {last}",
            read.channel,
            others.join(", "),
        ),
        "Give the channel one of these sample types, or remove its `read` block".into(),
    )
}

/// The error of a read of `reads` whose node alone needs a Read past one message of the
/// server.
fn large_read(reads: &[Read], Large { node, bytes, most }: Large) -> Error {
    let read = &reads[node];
    Error::Config(vec![Diagnostic::new(
        LARGE_READ,
        None,
        format!(
            "a Read of the node `{}` of the channel `{}` needs {bytes} bytes, past the \
             {most} bytes of one message of the server",
            read.node, read.channel,
        ),
        "Raise the message size of the server".into(),
    )])
}

/// Why a run ended with [`Error::Device`].
#[derive(Debug)]
enum Failure {
    Connect { address: String, status: Status },
    Read(Status),
    Check(Status),
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { address, status } => {
                write!(f, "the connect to {address} failed with {}", status.name())
            }
            Self::Read(status) => write!(f, "the read failed with {}", status.name()),
            Self::Check(status) => {
                write!(f, "the connectivity check failed with {}", status.name())
            }
        }
    }
}

impl std::error::Error for Failure {}

fn device(failure: Failure) -> Error {
    Error::Device(Box::new(failure))
}

/// `group` as a position.
fn to_usize(group: u32) -> usize {
    usize::try_from(group).expect("invariant: a usize holds a group")
}
