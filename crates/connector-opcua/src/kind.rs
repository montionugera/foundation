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
#[cfg(feature = "sim")]
use types::time::Span;
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
    let mut frames = Frames::open(&ctx).await?;
    let config = ctx.config();
    let (clock, net) = (ctx.clock().clone(), ctx.net().clone());
    let connect = Client::connect(clock, net, &mut ctx.rng(), &config.address).await;
    let mut client = connect.map_err(|status| {
        device(Failure::Connect {
            address: config.address.to_string_lossy().into_owned(),
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
    let mut missed = Total::new(ctx.count("missed"));
    let mut timer = pace::Timer::new(ctx.clock(), ctx.config().rate);
    while let Some(tick) = timer.tick(ctx.cancel()).await {
        missed.add(tick.missed);
        let Some((read, stamp)) =
            ctx.cancel().race(frames.request(client.read(&nodes))).await
        else {
            break;
        };
        let values = read.map_err(|status| device(Failure::Read(status)))?;
        frames.write(reads, values, stamp)?;
    }
    Ok(())
}

/// Writes `count` frames of the reads of `ctx` with no server, each value 1 and each
/// frame 1 ms of its clock after the last, and gives what `measure` gives for the
/// check of the values and the write of each frame. `measure` calls the work that it
/// gets once. For benchmarks.
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
    mut measure: impl FnMut(&mut dyn FnMut()) -> u64,
) -> Result<Vec<u64>, Error> {
    let mut frames = Frames::open(ctx).await?;
    let reads = &ctx.config().reads;
    let values = vec![Ok(1.0); reads.len()];
    let mut measures = Vec::with_capacity(count);
    for _ in 0..count {
        ctx.clock().sleep(Span::from_nanos(1_000_000)).await;
        let ((), stamp) = frames.request(future::ready(())).await;
        let mut written = Ok(());
        measures.push(measure(&mut || {
            written = frames.write(reads, &values, stamp);
        }));
        written?;
    }
    Ok(measures)
}

/// The frames of the ticks of a run: one sample in each series of the key set of its
/// writer, the stamp in each index and a value in each other.
struct Frames {
    layout: Layout,
    out: Out,
    stamps: Stamps,
    /// The series of the groups that a frame writes again.
    again: Vec<(usize, usize)>,
    /// The groups that the home refused as `Backwards`, in order.
    backwards: Vec<u32>,
}

/// The series of the frame of each tick, and what fills each.
struct Layout {
    /// Each entry of the key set and the bytes of its series, 8 for each.
    series: Vec<(usize, usize)>,
    /// The sample of each entry of the key set.
    fills: Vec<Fill>,
    /// The count of data samples of each group in one frame, by its number.
    samples: Vec<u64>,
    /// The group of each index, in the order of the key set.
    groups: Vec<u32>,
}

/// The writer of the frames, and the counts of their samples.
struct Out {
    writer: Writer,
    applied: Total,
    lost: Total,
}

impl Frames {
    /// Opens the writer of the channels of the reads of `ctx`, and gives its frames.
    ///
    /// # Errors
    ///
    /// The error of [`Context::writer`]. [`Error::Config`] with `opcua.sample-type`
    /// for each channel whose sample type is not `f64`.
    async fn open(ctx: &Context<Config>) -> Result<Self, Error> {
        let reads = &ctx.config().reads;
        let channels = reads.iter().map(|read| read.channel.clone()).collect();
        let writer = ctx.writer(channels, Authority::ABSOLUTE, None).await?;
        let set = writer.set();
        let entries = set.entries();
        let wrong = (reads.iter().zip(writer.entries()))
            .filter_map(|(read, &entry)| sample_type(read, entries[entry].data_type))
            .collect::<Vec<_>>();
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
        let series: Vec<_> = (0..entries.len()).map(|entry| (entry, 8)).collect();
        Ok(Self {
            again: Vec::with_capacity(series.len()),
            backwards: Vec::with_capacity(samples.len()),
            layout: Layout {
                series,
                fills,
                samples,
                groups,
            },
            out: Out {
                applied: Total::new(ctx.count("in")),
                lost: Total::new(ctx.count("lost")),
                writer,
            },
            stamps: Stamps::default(),
        })
    }

    /// Runs `request`, and gives its output with the stamp of its frame.
    async fn request<T>(&mut self, request: impl Future<Output = T>) -> (T, Stamp) {
        self.stamps.request(&self.out.writer, request).await
    }

    /// Writes one frame of `values`, the values of `reads` in order, at `stamp`. A
    /// group that the home refuses as `Backwards` is written again once, after the
    /// stamp that the refusal gives.
    ///
    /// # Errors
    ///
    /// [`Error::Device`] when a value is a bad status, with no write. As
    /// [`Out::send`].
    ///
    /// # Panics
    ///
    /// When the home refuses a group as `Backwards` again.
    fn write(
        &mut self,
        reads: &[Read],
        values: &[Result<f64, Status>],
        stamp: Stamp,
    ) -> Result<(), Error> {
        let values = Values::new(values).map_err(|(at, status)| {
            device(Failure::Value {
                channel: reads[at].channel.clone(),
                node: reads[at].node.clone(),
                status,
            })
        })?;
        let Self {
            layout,
            out,
            stamps,
            again,
            backwards,
        } = self;
        let (all, groups) = (&layout.series, &layout.groups);
        let Some(before) = out.send(layout, all, groups, values, stamp, backwards)?
        else {
            return Ok(());
        };
        let stamp = stamps.after(before);
        let refused = |&&(entry, _): &&(usize, usize)| {
            backwards.contains(&layout.fills[entry].group)
        };
        again.clear();
        again.extend(all.iter().filter(refused));
        let none = &mut Vec::new();
        if let Some(again) = out.send(layout, again, backwards, values, stamp, none)? {
            panic!(
                "invariant: a frame stamped after {before}, the last stamp of its \
                 index, is not backwards, but the home gives {again} as last"
            );
        }
        Ok(())
    }
}

impl Out {
    /// Writes one frame of `series` of `layout` with `values` at `stamp`, and counts
    /// its samples. `groups` are the groups of the indexes of `series`, in order. A
    /// frame for which the pool has no block now loses its samples.
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
        layout: &Layout,
        series: &[(usize, usize)],
        groups: &[u32],
        values: Values<'_>,
        stamp: Stamp,
        backwards: &mut Vec<u32>,
    ) -> Result<Option<Stamp>, Error> {
        let Layout { fills, samples, .. } = layout;
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
        let sample = match fills[entry].read {
            None => stamp.nanos().to_le_bytes(),
            Some(read) => values.get(read).to_le_bytes(),
        };
        bytes.copy_from_slice(&sample);
    }
    for &group in groups {
        draft.set_count(group, 1);
    }
}

/// The values of one read, each a good Double.
#[derive(Clone, Copy)]
struct Values<'a>(&'a [Result<f64, Status>]);

impl<'a> Values<'a> {
    /// Gives `values`, or the position and the status of the first bad one.
    fn new(values: &'a [Result<f64, Status>]) -> Result<Self, (usize, Status)> {
        match values
            .iter()
            .enumerate()
            .find_map(|(at, value)| Some((at, *value.as_ref().err()?)))
        {
            Some(bad) => Err(bad),
            None => Ok(Self(values)),
        }
    }

    /// The value of the read at `at`.
    fn get(self, at: usize) -> f64 {
        match self.0[at] {
            Ok(value) => value,
            Err(_) => self.broken(),
        }
    }

    /// Panics with the first bad read and its status. Cold and apart from `get`, so
    /// the loop of a send keeps no read on the stack for the message.
    #[cold]
    #[inline(never)]
    fn broken(self) -> ! {
        let bad = Self::new(self.0).err();
        panic!("invariant: `Values::new` refuses the read and status {bad:?}")
    }
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
        self.total += samples;
        self.count.set(self.total);
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
