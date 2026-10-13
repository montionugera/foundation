//! The `opcua` kind: reads OPC UA nodes at a fixed rate, and writes one frame per
//! tick.

use std::{fmt, future};

use connector::kind::{self, Channels, Context, Error};
use connector::stamp::Stamps;
use connector::{cancel, pace, status};
use document::Document;
use document::diagnostic::{Code, Diagnostic};
use hub::home::{Outcome, Refusal, order};
use hub::writer::Writer;
use types::authority::Authority;
use types::frame::{self, Draft, Form, Label, Path, key_set};
use types::name::Name;
use types::sample::{Scalar, Type};
use types::time::Stamp;

use crate::client::{Client, Large};
use crate::config::{self, Config, Read};
use crate::ffi::Status;
use crate::node::Node;

#[cfg(test)]
#[cfg(feature = "sim")]
mod tests;

const SAMPLE_TYPE: Code = Code::new("opcua.sample-type");
const LARGE_READ: Code = Code::new("opcua.large-read");

/// The counts of the status of each connector: the samples that the home applied,
/// the ticks that the timer skipped, and the samples that a frame lost.
const COUNTS: [&str; 3] = ["in", "missed", "lost"];

/// The `opcua` kind: reads the Value of OPC UA nodes at a fixed rate, one Read
/// request of all of them per tick, and writes one frame per tick.
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
    /// `in`, `missed`, and `lost`.
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
    /// the cancel. Each tick writes one frame, stamped at the midpoint of the read.
    /// The connect is not raced with the cancel: it ends in the request timeout of the
    /// client (5 s). Each end closes the session.
    ///
    /// # Errors
    ///
    /// The error of [`Context::writer`] and of each failed write. [`Error::Config`]
    /// with `opcua.sample-type` for each channel whose sample type is not `f64`, and
    /// with `opcua.large-read` when one Read of all nodes is past one message of the
    /// server. [`Error::Device`] when the connect or a read fails, or a node gives a
    /// bad status or a value that is not a Double.
    fn run(&self, ctx: Context<Config>) -> impl Future<Output = Result<(), Error>> {
        run(ctx)
    }
}

async fn run(ctx: Context<Config>) -> Result<(), Error> {
    let config = ctx.config();
    let channels = config
        .reads
        .iter()
        .map(|read| read.channel.clone())
        .collect();
    let writer = ctx.writer(channels, Authority::ABSOLUTE, None).await?;
    let mut frames = Frames::new(&ctx, writer)?;
    let (clock, net) = (ctx.clock().clone(), ctx.net().clone());
    let connect = Client::connect(clock, net, &mut ctx.rng(), &config.address).await;
    let mut client = connect.map_err(|status| {
        device(Failure::Connect {
            address: config.address.clone(),
            status,
        })
    })?;
    let result = read(&ctx, &mut client, &mut frames).await;
    client.close().await;
    result
}

/// Reads each node once per tick until the cancel, and writes one frame per tick.
async fn read(
    ctx: &Context<Config>,
    client: &mut Client,
    frames: &mut Frames,
) -> Result<(), Error> {
    let reads = &ctx.config().reads;
    let nodes: Vec<Node> = reads.iter().map(|read| read.node.clone()).collect();
    let nodes = client
        .nodes(&nodes)
        .map_err(|large| large_read(reads.len(), large))?;
    let mut values = Vec::with_capacity(reads.len());
    let mut timer = pace::Timer::new(ctx.clock(), ctx.config().rate);
    while let Some(tick) = timer.tick(ctx.cancel()).await {
        frames.missed.add(tick.missed);
        let request = frames.stamps.request(&frames.writer, client.read(&nodes));
        let Some((read, stamp)) = ctx.cancel().race(request).await else {
            break;
        };
        let read = read.map_err(|status| device(Failure::Read(status)))?;
        values.clear();
        for (of, value) in reads.iter().zip(read) {
            let value = value.map_err(|status| {
                device(Failure::Value {
                    channel: of.channel.clone(),
                    node: of.node.clone(),
                    status,
                })
            })?;
            values.push(value);
        }
        frames.write(&values, stamp)?;
    }
    Ok(())
}

/// The frame of each tick of a run: one sample in each series of the key set of its
/// writer, the stamp in each index and a value in each other.
struct Frames {
    writer: Writer,
    stamps: Stamps,
    /// Each entry of the key set and the bytes of its series, 8 for each.
    series: Vec<(usize, usize)>,
    /// The sample of each entry of the key set.
    fills: Vec<Fill>,
    /// The values of each group, by its number.
    samples: Vec<u64>,
    /// The series of the groups that a frame wrote again.
    again: Vec<(usize, usize)>,
    /// The groups that the home refused as `Backwards`, in order.
    backwards: Vec<u32>,
    applied: Total,
    lost: Total,
    missed: Total,
}

impl Frames {
    /// The frames of `writer`, whose channels are those of the reads of `ctx`.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `opcua.sample-type` for each channel whose sample type
    /// is not `f64`.
    fn new(ctx: &Context<Config>, writer: Writer) -> Result<Self, Error> {
        let set = writer.set();
        let entries = set.entries();
        let reads = &ctx.config().reads;
        let wrong = (reads.iter().zip(writer.entries()))
            .filter_map(|(read, &entry)| sample_type(read, entries[entry].data_type))
            .collect::<Vec<_>>();
        if !wrong.is_empty() {
            return Err(Error::Config(wrong));
        }
        let mut samples = vec![0; set.groups().len()];
        let fills = (entries.iter().enumerate())
            .map(|(entry, &key_set::Entry { group, .. })| {
                if set.groups().contains(&entry) {
                    return Fill { group, read: None };
                }
                let read = writer.entries().iter().position(|&of| of == entry);
                let read = read.expect("invariant: a writer has only its channels");
                samples[to_usize(group)] += 1;
                Fill {
                    group,
                    read: Some(read),
                }
            })
            .collect();
        let series: Vec<_> = (0..entries.len()).map(|entry| (entry, 8)).collect();
        let total = |name| Total::new(ctx.count(name));
        Ok(Self {
            again: Vec::with_capacity(series.len()),
            backwards: Vec::with_capacity(samples.len()),
            series,
            fills,
            samples,
            applied: total("in"),
            lost: total("lost"),
            missed: total("missed"),
            writer,
            stamps: Stamps::default(),
        })
    }

    /// Writes one frame of `values`, in the order of the reads, at `stamp`. A group
    /// that the home refuses as `Backwards` is written again once, after the stamp
    /// that the refusal gives.
    ///
    /// # Errors
    ///
    /// As [`Frames::send`].
    ///
    /// # Panics
    ///
    /// When the home refuses a group as `Backwards` again.
    fn write(&mut self, values: &[f64], stamp: Stamp) -> Result<(), Error> {
        let Some(before) = self.send(Series::All, values, stamp)? else {
            return Ok(());
        };
        let stamp = self.stamps.after(before);
        let (fills, backwards) = (&self.fills, &self.backwards);
        let refused =
            |&&(entry, _): &&(usize, usize)| backwards.contains(&fills[entry].group);
        self.again.clear();
        self.again.extend(self.series.iter().filter(refused));
        if let Some(again) = self.send(Series::Again, values, stamp)? {
            panic!(
                "invariant: a frame stamped after {before}, the last stamp of its \
                 index, is not backwards, but the home gives {again} as last"
            );
        }
        Ok(())
    }

    /// Writes one frame of `series` of `values` at `stamp`, and counts its samples.
    /// A frame for which the pool has no block now loses its samples.
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
        series: Series,
        values: &[f64],
        stamp: Stamp,
    ) -> Result<Option<Stamp>, Error> {
        let Self {
            writer,
            series: all,
            again,
            fills,
            samples,
            backwards,
            applied,
            lost,
            ..
        } = self;
        let series = match series {
            Series::All => &*all,
            Series::Again => &*again,
        };
        let fills = &*fills;
        let groups = || {
            let indexes = series.iter().map(|&(entry, _)| &fills[entry]);
            indexes
                .filter(|fill| fill.read.is_none())
                .map(|fill| fill.group)
        };
        let mut draft = match writer.draft(Form::Raw, series) {
            Ok(draft) => draft,
            Err(frame::Error::Pool(
                block::Error::Exhausted { .. } | block::Error::Refused { .. },
            )) => {
                lost.add(groups().map(|group| samples[to_usize(group)]).sum());
                return Ok(None);
            }
            Err(frame::Error::Pool(error)) => return Err(error.into()),
            Err(error) => panic!("invariant: the series follow the key set: {error}"),
        };
        fill(&mut draft, fills, values, stamp, groups());
        let outcomes = writer.write(Label::Path(Path::Live), draft)?;
        backwards.clear();
        let mut latest = None;
        let (mut wrote, mut gone) = (0, 0);
        for (group, outcome) in groups().zip(outcomes) {
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
                    "the home refuses a frame with no lease in raw form: {refusal}"
                ),
            }
        }
        applied.add(wrote);
        lost.add(gone);
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
    values: &[f64],
    stamp: Stamp,
    groups: impl Iterator<Item = u32>,
) {
    for (entry, bytes) in draft.iter_mut() {
        let sample = match fills[entry].read {
            None => stamp.nanos().to_le_bytes(),
            Some(read) => values[read].to_le_bytes(),
        };
        bytes.copy_from_slice(&sample);
    }
    for group in groups {
        draft.set_count(group, 1);
    }
}

/// Which series [`Frames::send`] writes.
#[derive(Clone, Copy)]
enum Series {
    /// Each series of the key set.
    All,
    /// The series of the groups that the home refused as `Backwards`.
    Again,
}

/// A count of the status and its total in the run.
struct Total {
    count: status::Count,
    total: u64,
}

impl Total {
    fn new(count: status::Count) -> Self {
        Self { count, total: 0 }
    }

    fn add(&mut self, samples: u64) {
        if samples > 0 {
            self.total += samples;
            self.count.set(self.total);
        }
    }
}

/// The diagnostic of the channel of `read` when its sample type is not `f64`.
fn sample_type(read: &Read, data_type: Type) -> Option<Diagnostic> {
    (data_type != Type::Scalar(Scalar::F64)).then(|| {
        Diagnostic::new(
            SAMPLE_TYPE,
            None,
            format!(
                "the channel `{}` has samples of type {data_type}, and the opcua kind \
                 reads only f64",
                read.channel
            ),
            "Give the channel the sample type f64, or remove its `read` block".into(),
        )
    })
}

/// The error of a Read of `nodes` nodes past one message of the server.
fn large_read(nodes: usize, Large { bytes, most }: Large) -> Error {
    Error::Config(vec![Diagnostic::new(
        LARGE_READ,
        None,
        format!(
            "a Read of {nodes} nodes needs {bytes} bytes, past the {most} bytes of one \
             message of the server"
        ),
        "Read fewer nodes in the connector, or split it into two connectors".into(),
    )])
}

/// Why a run ended with [`Error::Device`].
#[derive(Debug)]
enum Failure {
    Connect {
        address: String,
        status: Status,
    },
    Read(Status),
    Value {
        channel: Name,
        node: Node,
        status: Status,
    },
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect { address, status } => {
                write!(f, "the connect to {address} failed with {}", status.name())
            }
            Self::Read(status) => write!(f, "the read failed with {}", status.name()),
            Self::Value {
                channel,
                node,
                status,
            } => write!(f, "the node {node} of `{channel}` gives {}", status.name()),
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
