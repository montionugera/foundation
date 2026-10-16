//! The `opcua` kind: reads OPC UA nodes at a fixed rate, and writes at most one frame
//! per tick.

use std::fmt;

use connector::kind::{self, Channels, Context, Error};
use connector::{pace, poll, status};
use document::Document;
use document::diagnostic::{Code, Diagnostic};
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
const COUNTS: [&str; 6] = {
    let [applied, lost] = poll::COUNTS;
    [applied, "missed", lost, "bad", "uncertain", "mistyped"]
};

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
    /// with `connector.index` for each channel that is an index; that error comes
    /// alone. Else [`Error::Config`] with `opcua.sample-type` for each channel whose
    /// sample type is not `bool`, `i8` to `u64`, `f32`, or `f64`, and with
    /// `opcua.large-read` when the Read of one node alone is past one message of the
    /// server. [`Error::Device`] when the connect, a read, or one check fails: a check
    /// fails on a bad service result or on no answer in 5 s. Also [`Error::Device`],
    /// at once, when the client loses its channel or its session between two ticks.
    fn run(&self, ctx: Context<Config>) -> impl Future<Output = Result<(), Error>> {
        run(ctx)
    }
}

async fn run(ctx: Context<Config>) -> Result<(), Error> {
    let mut frames = Frames::open(&ctx).await?;
    let config = ctx.config();
    let mesh = frames.writer.time();
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
        let Some((read, stamp)) = ctx
            .cancel()
            .race(frames.writer.request(client.read(&nodes)))
            .await
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
        let types = frames.types.iter().enumerate();
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
        let ((), stamp) = frames.writer.request(std::future::ready(())).await;
        let mut written = Ok(());
        measures.push(measure(&mut || {
            written = frames.write(&values, stamp);
        }));
        written?;
    }
    Ok(measures)
}

/// The frames of the ticks of a run: the value of each read that is Good and of the
/// sample type of its channel.
struct Frames<'a> {
    /// The context of the run, whose reads give the values of each frame, in order.
    ctx: &'a Context<Config>,
    writer: poll::Writer,
    /// The sample type of the channel of each read, in order.
    types: Vec<Scalar>,
    left: Left,
}

/// The counts of the values that a frame leaves out.
struct Left {
    bad: status::Count,
    uncertain: status::Count,
    mistyped: status::Count,
    /// The read and the type that the note names, or `None` when it shows none.
    noted: Option<(usize, value::Type)>,
}

impl<'a> Frames<'a> {
    /// Opens the writer of the channels of the reads of `ctx`, and gives its frames.
    ///
    /// # Errors
    ///
    /// The error of [`poll::Writer::open`], which comes alone. Else [`Error::Config`]
    /// with `opcua.sample-type` for each channel whose sample type is not one of
    /// [`SCALARS`].
    async fn open(ctx: &'a Context<Config>) -> Result<Self, Error> {
        let reads = &ctx.config().reads;
        let channels = reads.iter().map(|read| read.channel.clone()).collect();
        let writer = poll::Writer::open(ctx, channels).await?;
        let (mut types, mut wrong) = (Vec::with_capacity(reads.len()), Vec::new());
        for (read, data_type) in reads.iter().zip(writer.types()) {
            match data_type {
                Type::Scalar(scalar) if SCALARS.contains(&scalar) => types.push(scalar),
                _ => wrong.push(sample_type(read, data_type)),
            }
        }
        if !wrong.is_empty() {
            return Err(Error::Config(wrong));
        }
        Ok(Self {
            ctx,
            writer,
            types,
            left: Left {
                bad: ctx.count("bad"),
                uncertain: ctx.count("uncertain"),
                mistyped: ctx.count("mistyped"),
                noted: None,
            },
        })
    }

    /// Writes one frame of `values`, the values of the reads in order, at `stamp`,
    /// with each value that is Good and of the sample type of its channel. It counts
    /// each other value in [`Left`], and the note of the context names the first node
    /// of another type.
    ///
    /// # Errors
    ///
    /// As [`poll::Writer::write`].
    ///
    /// # Panics
    ///
    /// When `values` does not have one value for each read.
    fn write(
        &mut self,
        values: &[Result<Value, Fault>],
        stamp: Stamp,
    ) -> Result<(), Error> {
        let (mut bad, mut uncertain, mut mistyped) = (0, 0, 0);
        let mut first = None;
        let kept = values.iter().enumerate().map(|(at, value)| {
            let sample = self.types[at];
            match Why::of(value, sample) {
                Ok(value) => Some(value),
                Err(Why::Bad) => {
                    bad += 1;
                    None
                }
                Err(Why::Uncertain) => {
                    uncertain += 1;
                    None
                }
                Err(Why::Mistyped(value)) => {
                    mistyped += 1;
                    first = first.or(Some((at, value, sample)));
                    None
                }
            }
        });
        // `write` takes each value before it returns, so the counts are whole after it.
        let written = self.writer.write(kept, stamp);
        self.left.add(bad, uncertain, mistyped);
        self.left.note(self.ctx, first);
        written
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
    /// The sample that a frame keeps of `value`, whose channel has samples of type
    /// `sample`, or why the frame leaves it out.
    fn of(value: &Result<Value, Fault>, sample: Scalar) -> Result<sample::Value, Self> {
        match value {
            Ok(value) if value.scalar() == sample => Ok(sample::Value::from(*value)),
            Ok(value) => Err(Self::Mistyped(value.type_of())),
            Err(fault) => Err(match fault.cause() {
                Cause::Type(opc_type) => Self::Mistyped(opc_type),
                Cause::Status(status) if status.uncertain() => Self::Uncertain,
                Cause::Status(_) => Self::Bad,
            }),
        }
    }
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
fn large_read(reads: &[Read], Large { index, bytes, most }: Large) -> Error {
    let read = &reads[index];
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
