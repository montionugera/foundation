//! The `modbus` kind: reads the tables of a Modbus TCP device at a fixed rate, and
//! writes one frame per tick.

use std::fmt;
use std::net::SocketAddr;

use connector::kind::{self, Channels, Context, Error};
use connector::{pace, poll};
use document::Document;
use document::diagnostic::{Code, Diagnostic};
use env::net::host::Host;
use env::net::{self, tcp};
use types::sample::{Type, Value};
use types::time::Span;

use crate::config::{self, Config};
use crate::pdu::{Exception, Request, Table};
use crate::read::{self, Group};
use crate::tcp::{Client, Failure};

#[cfg(test)]
mod tests;

/// The counts of the status of each connector: the samples that the home applied,
/// the ticks that the timer skipped, and the samples that a frame lost.
const COUNTS: [&str; 3] = {
    let [applied, lost] = poll::COUNTS;
    [applied, "missed", lost]
};

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
        let mut writer = poll::Writer::open(&ctx, channels.collect()).await?;
        let groups = groups(config, writer.types())?;
        let Some(client) = ctx.cancel().race(connect(&ctx)).await else {
            return Ok(());
        };
        let mut client = client?;
        let mut values = vec![None; config.reads.len()];
        let missed = ctx.count("missed");
        let mut timer = pace::Timer::new(ctx.clock(), config.rate);
        while let Some(tick) = timer.tick(ctx.cancel()).await {
            missed.add(tick.missed);
            let read = read(config, &mut client, &groups, &mut values);
            let Some((read, stamp)) = ctx.cancel().race(writer.request(read)).await
            else {
                break;
            };
            read?;
            writer.write(values.iter().copied(), stamp)?;
        }
        Ok(())
    }
}

/// The time that one exchange waits for its reply.
const TIMEOUT: Span = Span::SECOND;

const SAMPLE_TYPE: Code = Code::new("modbus.sample-type");
const RANGE: Code = Code::new("modbus.range");
const EXCEPTION: Code = Code::new("modbus.exception");

/// The requests of the reads of `config`, whose channels have the sample types
/// `types`, in order.
///
/// # Errors
///
/// [`Error::Config`] with a diagnostic of each read that is not valid.
fn groups(
    config: &Config,
    types: impl Iterator<Item = Type>,
) -> Result<Vec<Group>, Error> {
    let (mut reads, mut wrong) = (Vec::with_capacity(config.reads.len()), Vec::new());
    for (read, sample) in config.reads.iter().zip(types) {
        match read::Read::new(read.table, read.address, sample) {
            Ok(valid) => reads.push(valid),
            Err(error) => {
                let (code, fix) = match error {
                    read::Error::Type { .. } => (
                        SAMPLE_TYPE,
                        "Give the channel a sample type that its table holds",
                    ),
                    read::Error::Range { .. } => (
                        RANGE,
                        "Give the read a lower address, or the channel a type of fewer \
                         registers",
                    ),
                    read::Error::ReadOnly { .. } => {
                        unreachable!("invariant: `Read::new` gives no `ReadOnly`")
                    }
                };
                let message = format!("{}: {error}", at(read));
                wrong.push(Diagnostic::new(code, None, message, fix.into()));
            }
        }
    }
    if !wrong.is_empty() {
        return Err(Error::Config(wrong));
    }
    Ok(read::groups(&reads, config.order))
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
/// [`Error::Device`] when the lookup fails, or with the connect error of the first
/// address when none takes the stream.
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
    values: &mut [Option<Value>],
) -> Result<(), Error> {
    for group in groups {
        let reply = client.exchange(config.unit, group.request()).await;
        let reply = reply
            .map_err(|failure| device(Fault::Exchange(Items::of(group), failure)))?;
        let read = group
            .values(reply)
            .map_err(|exception| refused(config, group, exception))?;
        for (at, value) in read {
            let slot = values.get_mut(at);
            *slot.expect("invariant: a group reads only the reads of `config`") =
                Some(value);
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
                    let read = config.reads.get(read);
                    let read = read
                        .expect("invariant: a group reads only the reads of `config`");
                    let message = format!(
                        "the device refused the read of {} with exception {exception}",
                        at(read)
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
        host: Host,
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
