//! The `modbus` kind: reads the tables of a Modbus TCP device at a fixed rate, and
//! writes one frame per tick.

use std::net::SocketAddr;
use std::{fmt, future};

use connector::kind::{self, Channels, Context, Error};
use connector::stamp::Stamps;
use connector::{cancel, pace, status};
use document::Document;
use document::diagnostic::{Code, Diagnostic};
use env::net::{self, tcp};
use hub::home::{Outcome, Refusal, order};
use hub::writer::Writer;
use types::authority::Authority;
use types::frame::{self, Draft, Form, Label, Path};
use types::sample::Value;
use types::time::{Span, Stamp};

use crate::config::{self, Config};
use crate::pdu::{Exception, Request, Table};
use crate::poll::{self, Group};
use crate::tcp::{Client, Failure};

#[cfg(test)]
mod tests;

/// The counts of the status of each connector: the samples that the home applied,
/// the ticks that the timer skipped, and the samples that a frame lost.
const COUNTS: [&str; 3] = ["in", "missed", "lost"];

/// The `modbus` kind: reads the tables of a Modbus TCP device at a fixed rate, and
/// writes one frame per tick.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Kind;

impl kind::Kind for Kind {
    type Config = Config;

    /// Reads `address`, `rate`, the optional `unit` and `order`, and one or more
    /// `read` blocks, each with the channel as its label, a `table`, and an
    /// `address`, and no other key or block.
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

    /// Finds nothing: the kind does not scan a network.
    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        future::ready(Ok(Vec::new()))
    }

    /// Connects to the device and reads each `read` once per tick until the cancel,
    /// in one request for the reads of one table that touch or overlap, and writes
    /// one frame per tick, stamped at the midpoint of the mesh times of the requests.
    /// Each exchange gives up after 1 s.
    ///
    /// # Errors
    ///
    /// The error of [`Context::writer`] and of each failed write. [`Error::Config`]
    /// with `modbus.sample-type` for each read whose channel has a type that its
    /// table does not hold, `modbus.range` for each read whose value runs past
    /// address 65535, and `modbus.exception` for each read of a request that the
    /// device refuses with exception 1, 2, or 3. [`Error::Device`] when the lookup or
    /// the connect fails, an exchange fails, or the device refuses a request with
    /// another exception.
    async fn run(&self, ctx: Context<Config>) -> Result<(), Error> {
        let config = ctx.config();
        let channels = config.reads.iter().map(|read| read.channel.clone());
        let writer = ctx
            .writer(channels.collect(), Authority::ABSOLUTE, None)
            .await?;
        let groups = groups(config, &writer)?;
        let Some(client) = ctx.cancel().race(connect(&ctx)).await else {
            return Ok(());
        };
        let mut client = client?;
        let mut frames = Frames::new(&ctx, writer);
        let mut values = vec![Value::Bool(false); config.reads.len()];
        let missed = ctx.count("missed");
        let mut timer = pace::Timer::new(ctx.clock(), config.rate);
        while let Some(tick) = timer.tick(ctx.cancel()).await {
            missed.add(tick.missed);
            let read = read(config, &mut client, &groups, &mut values);
            let Some((read, stamp)) = ctx.cancel().race(frames.request(read)).await
            else {
                break;
            };
            read?;
            frames.write(&values, stamp)?;
        }
        Ok(())
    }
}

/// The time that one exchange waits for its reply.
const TIMEOUT: Span = Span::SECOND;

const SAMPLE_TYPE: Code = Code::new("modbus.sample-type");
const RANGE: Code = Code::new("modbus.range");
const EXCEPTION: Code = Code::new("modbus.exception");

/// The requests of the reads of `config`, whose channels have the sample types of
/// `writer`.
///
/// # Errors
///
/// [`Error::Config`] with a diagnostic of each read that is not valid.
fn groups(config: &Config, writer: &Writer) -> Result<Vec<Group>, Error> {
    let entries = writer.set().entries();
    let (mut reads, mut wrong) = (Vec::with_capacity(config.reads.len()), Vec::new());
    for (read, &entry) in config.reads.iter().zip(writer.entries()) {
        let sample = entries[entry].data_type;
        match poll::Read::new(read.table, read.address, sample) {
            Ok(valid) => reads.push(valid),
            Err(error) => {
                let (code, fix) = match error {
                    poll::Error::Type { .. } => (
                        SAMPLE_TYPE,
                        "Give the channel a sample type that its table holds",
                    ),
                    poll::Error::Range { .. } => (
                        RANGE,
                        "Give the read a lower address, or the channel a type of fewer \
                         registers",
                    ),
                };
                let message = format!("{}: {error}", at(read));
                wrong.push(Diagnostic::new(code, None, message, fix.into()));
            }
        }
    }
    if !wrong.is_empty() {
        return Err(Error::Config(wrong));
    }
    Ok(poll::groups(&reads, config.order))
}

/// The channel, the address, and the table of `read`, for a diagnostic.
fn at(read: &config::Read) -> String {
    format!(
        "the channel `{}` at address {} of the {}",
        read.channel,
        read.address,
        read.table.name()
    )
}

/// Connects to the first address of the host of `config` that takes the stream.
///
/// # Errors
///
/// [`Error::Device`] when the lookup fails, or with the connect error of the first address when none takes the stream.
async fn connect(ctx: &Context<Config>) -> Result<Client, Error> {
    let config = ctx.config();
    let host = &config.host;
    let remotes = ctx.net().resolve(host, config.port).await;
    let remotes = remotes.map_err(|error| {
        device(Fault::Lookup {
            host: host.clone(),
            error,
        })
    })?;
    let mut first = None;
    for &remote in &remotes {
        let stream = tcp::Config {
            remote,
            options: OPTIONS,
        };
        match Client::connect(ctx.net(), &stream, ctx.clock().clone(), TIMEOUT).await {
            Ok(client) => return Ok(client),
            Err(error) => {
                first.get_or_insert((remote, error));
            }
        }
    }
    let (remote, error) = first.expect("invariant: a lookup gives an address");
    Err(device(Fault::Connect { remote, error }))
}

/// The options of the stream to a device: a request and its reply are each at most
/// 260 bytes.
const OPTIONS: tcp::Options = tcp::Options {
    send_buffer_bytes: 1 << 12,
    recv_buffer_bytes: 1 << 12,
    unsent_bytes_max: std::num::NonZeroUsize::new(1 << 12).expect("not 0"),
    delayed: false,
};

/// Sends the request of each of `groups` in turn, and puts the value of each read
/// of its reply into `values`, in the order of the reads of `config`.
///
/// # Errors
///
/// As [`Kind::run`](kind::Kind::run), for an exchange.
async fn read(
    config: &Config,
    client: &mut Client,
    groups: &[Group],
    values: &mut [Value],
) -> Result<(), Error> {
    for group in groups {
        let reply = client.exchange(config.unit, group.request()).await;
        let reply = reply
            .map_err(|failure| device(Fault::Exchange(Items::of(group), failure)))?;
        let read = group
            .values(reply)
            .map_err(|exception| refused(config, group, exception))?;
        for (at, value) in read {
            values[at] = value;
        }
    }
    Ok(())
}

/// The error of `group`, which the device refused with `exception`.
fn refused(config: &Config, group: &Group, exception: Exception) -> Error {
    match exception {
        Exception::IllegalFunction
        | Exception::IllegalAddress
        | Exception::IllegalValue => Error::Config(
            group
                .reads()
                .map(|read| {
                    let message = format!(
                        "the device refused the read of {} with exception {exception}",
                        at(&config.reads[read])
                    );
                    let fix = "Read a table and addresses that the device has";
                    Diagnostic::new(EXCEPTION, None, message, fix.into())
                })
                .collect(),
        ),
        _ => device(Fault::Exception(Items::of(group), exception)),
    }
}

/// Why a run ended with [`Error::Device`].
#[derive(Debug)]
enum Fault {
    Lookup {
        host: Box<str>,
        error: net::Error,
    },
    Connect {
        remote: SocketAddr,
        error: net::Error,
    },
    Exchange(Items, Failure),
    Exception(Items, Exception),
}

/// The items that one request reads: its table and its first address.
#[derive(Debug)]
struct Items {
    table: Table,
    start: u16,
}

impl Items {
    fn of(group: &Group) -> Self {
        let &Request::Read { table, start, .. } = group.request() else {
            unreachable!("invariant: a group holds a read request");
        };
        Self { table, start }
    }
}

impl fmt::Display for Items {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} from address {}", self.table.name(), self.start)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Lookup { host, error } => {
                write!(f, "the lookup of {host} failed: {error}")
            }
            Self::Connect { remote, error } => {
                write!(f, "the connect to {remote} failed: {error}")
            }
            Self::Exchange(items, failure) => {
                write!(f, "the read of {items} failed: {failure}")
            }
            Self::Exception(items, exception) => write!(
                f,
                "the device refused the read of {items} with exception {exception}"
            ),
        }
    }
}

impl std::error::Error for Fault {}

fn device(fault: Fault) -> Error {
    Error::Device(Box::new(fault))
}

/// The frames of a run: one sample in each series of the key set of its writer, the
/// stamp in each index and the value of a read in each other.
struct Frames {
    writer: Writer,
    stamps: Stamps,
    /// The read of each entry of the key set, or `None` for an index.
    reads: Vec<Option<usize>>,
    /// Each entry of the key set and the bytes of its sample.
    series: Vec<(usize, usize)>,
    /// The number of reads of each group, by its number.
    samples: Vec<u64>,
    /// The groups of the frame to write: each group, or those that the home refused
    /// as `Backwards`.
    groups: Vec<u32>,
    /// The series of `groups`.
    again: Vec<(usize, usize)>,
    applied: status::Count,
    lost: status::Count,
}

impl Frames {
    /// The frames of `writer`, whose channels are those of the reads of `ctx`, each of
    /// a type that its table holds.
    fn new(ctx: &Context<Config>, writer: Writer) -> Self {
        let set = writer.set();
        let entries = set.entries();
        let mut reads = vec![None; entries.len()];
        let mut samples = vec![0; set.groups().len()];
        for (read, &entry) in writer.entries().iter().enumerate() {
            reads[entry] = Some(read);
            samples[to_usize(entries[entry].group)] += 1;
        }
        let width = |entry: &frame::key_set::Entry| {
            (entry.data_type.width())
                .expect("invariant: an index or a read is a scalar")
        };
        let series: Vec<_> = entries.iter().map(width).enumerate().collect();
        let groups = set.groups().iter().map(|&index| entries[index].group);
        Self {
            again: Vec::with_capacity(series.len()),
            groups: groups.collect(),
            reads,
            series,
            samples,
            stamps: Stamps::default(),
            applied: ctx.count("in"),
            lost: ctx.count("lost"),
            writer,
        }
    }

    /// Runs `request`, and gives its output with the stamp of its frame.
    async fn request<T>(&mut self, request: impl Future<Output = T>) -> (T, Stamp) {
        self.stamps.request(&self.writer, request).await
    }

    /// Writes one frame of `values`, the values of the reads in order, at `stamp`. A
    /// group that the home refuses as `Backwards` is written again once, after the
    /// stamp that the refusal gives.
    ///
    /// # Errors
    ///
    /// As [`Frames::send`].
    ///
    /// # Panics
    ///
    /// When the home refuses a group as `Backwards` again.
    fn write(&mut self, values: &[Value], stamp: Stamp) -> Result<(), Error> {
        let set = self.writer.set();
        let all = set.groups().iter().map(|&index| set.entries()[index].group);
        self.groups.clear();
        self.groups.extend(all);
        self.again.clear();
        self.again.extend_from_slice(&self.series);
        let Some(before) = self.send(values, stamp)? else {
            return Ok(());
        };
        let stamp = self.stamps.after(before);
        let entries = self.writer.set().entries();
        let groups = &self.groups;
        let refused =
            |&&(entry, _): &&(usize, usize)| groups.contains(&entries[entry].group);
        let series = self.series.iter().filter(refused);
        self.again.clear();
        self.again.extend(series);
        if let Some(again) = self.send(values, stamp)? {
            panic!(
                "invariant: a frame stamped after {before}, the last stamp of its \
                 index, is not backwards, but the home gives {again} as last"
            );
        }
        Ok(())
    }

    /// Writes the series `again` of the groups `groups` at `stamp`, and counts the
    /// samples of each group as applied or lost. A frame for which the pool has no
    /// block now loses its samples. Leaves in `groups` each group that the home
    /// refused as `Backwards`, and gives the latest last stamp of one, or `None`.
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
    fn send(&mut self, values: &[Value], stamp: Stamp) -> Result<Option<Stamp>, Error> {
        let mut draft = match self.writer.draft(Form::Raw, &self.again) {
            Ok(draft) => draft,
            Err(frame::Error::Pool(
                block::Error::Exhausted { .. } | block::Error::Refused { .. },
            )) => {
                let lost = self.groups.iter().map(|&group| self.count(group)).sum();
                self.lost.add(lost);
                self.groups.clear();
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        fill(&mut draft, &self.reads, values, stamp, &self.groups);
        let outcomes = self.writer.write(Label::Path(Path::Live), draft)?;
        let mut latest = None;
        let (mut wrote, mut gone) = (0, 0);
        let mut backwards = 0;
        for (at, outcome) in outcomes.iter().enumerate() {
            let group = self.groups[at];
            let n = self.samples[to_usize(group)];
            match outcome {
                Outcome::Applied { .. } => wrote += n,
                Outcome::Refused {
                    refusal: Refusal::Order(order::Error::Backwards { before, .. }),
                    ..
                } => {
                    self.groups[backwards] = group;
                    backwards += 1;
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
        self.groups.truncate(backwards);
        self.applied.add(wrote);
        self.lost.add(gone);
        Ok(latest)
    }

    /// The number of reads of `group`.
    fn count(&self, group: u32) -> u64 {
        self.samples[to_usize(group)]
    }
}

/// Fills each series of `draft` with one sample: `stamp` in each index, and the value
/// of its read in each other, and gives each of `groups` one sample.
fn fill(
    draft: &mut Draft,
    reads: &[Option<usize>],
    values: &[Value],
    stamp: Stamp,
    groups: &[u32],
) {
    for (entry, bytes) in draft.iter_mut() {
        match reads[entry] {
            None => Value::Stamp(stamp).write(bytes),
            Some(read) => values[read].write(bytes),
        }
    }
    for &group in groups {
        draft.set_count(group, 1);
    }
}

/// `group` as a position.
fn to_usize(group: u32) -> usize {
    usize::try_from(group).expect("invariant: a usize holds a group")
}
