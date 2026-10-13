//! The status of each connector on a node, read from its status channels.

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

use hub::reader::{self, Config, Mode, Received};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use spec::channel::{Channel, Kind};
use spec::connector::Connector;
use spec::definition::Definition;
use types::name::{Name, Selector};
use types::sample::{Scalar, Type};
use types::time::{Monotonic, Span};

use crate::Node;
use crate::error::{Error, escape_controls};

#[cfg(test)]
mod tests;

/// The longest that `status` waits for the frames, from the request.
const WAIT: Span = Span::SECOND;

/// The status of each connector on the node, as `status` gives it.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Output {
    /// Each connector that the spec in use places on the node, and whose status the
    /// subject may read, in name order.
    pub(crate) connectors: Vec<Status>,
}

/// The status of one connector.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Status {
    /// The name of the connector.
    pub(crate) name: String,
    /// The kind of the connector.
    pub(crate) kind: String,
    pub(crate) state: State,
    /// The `address` attribute of the config, or `None` when it has none.
    pub(crate) address: Option<String>,
    /// The runs that the supervisor started again, or `None` when `state` is
    /// `unknown`.
    pub(crate) restarts: Option<u64>,
    /// Each count that the kind names, by name, or `None` when `state` is `unknown`.
    pub(crate) counts: Option<BTreeMap<String, u64>>,
    /// The error that ended the last run, or `None` when the last run returned `Ok`,
    /// no run ended, or `state` is `unknown`.
    pub(crate) error: Option<Failure>,
}

/// What the run of a connector does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum State {
    Running,
    Restarting,
    Stopped,
    Ending,
    /// No frame of the status came within a second, the newest frame holds no
    /// status, the hub does not hold the status channels of the spec yet, or an apply
    /// removed them: the supervisor has not started the connector, or it ended.
    Unknown,
}

/// The error that ended the last run of a connector.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Failure {
    pub(crate) class: Class,
    /// The text of the error.
    pub(crate) text: String,
}

/// The class of the error that ended a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Class {
    Config,
    Device,
    Retry,
}

/// A connector on the node, and its status channels as the spec defines them.
pub(crate) struct Placed<'a> {
    name: &'a Name,
    connector: &'a Connector,
    channels: Vec<(&'a Name, &'a Channel)>,
}

impl Placed<'_> {
    /// The names of the status channels, the index too.
    pub(crate) fn channels(&self) -> impl Iterator<Item = &Name> {
        self.channels.iter().map(|(name, _)| *name)
    }

    /// Whether the channels of `received` are the status channels of the spec, each
    /// with the key and the sample type that the spec gives its name. The frame holds
    /// its index, so this checks the index too. Each write of the supervisor holds
    /// every status channel, so a frame with fewer is of a hub that lacks one, or of a
    /// run of an earlier spec with fewer counts.
    fn defines(&self, received: &Received<'_>) -> bool {
        let (set, view) = (received.set(), received.view());
        view.iter().count() == self.channels.len()
            && view.iter().all(|(entry, _)| {
                let found = &set.entries()[entry];
                let name = received.name(entry);
                let mut defined = self.channels.iter();
                defined
                    .find(|(defined, _)| defined.as_str() == name)
                    .is_some_and(|(_, channel)| {
                        channel.key == found.key && sample(channel) == found.data_type
                    })
            })
    }
}

/// The type of the samples of `channel`. `hub` maps it the same way, until #2532
/// makes one mapping in `spec`.
fn sample(channel: &Channel) -> Type {
    match &channel.kind {
        Kind::Index { .. } => Type::Scalar(Scalar::Stamp),
        Kind::Data(data) => data.data_type().sample(),
    }
}

/// Each connector of `definitions` on the node named `node`, in name order.
pub(crate) fn placed<'a>(
    definitions: &'a BTreeMap<Name, Definition>,
    node: &Name,
) -> Vec<Placed<'a>> {
    definitions
        .iter()
        .filter_map(|(name, definition)| match definition {
            Definition::Connector(connector) if connector.node() == node => {
                Some(Placed {
                    name,
                    connector,
                    channels: connector::status::defined(definitions, name),
                })
            }
            _ => None,
        })
        .collect()
}

/// The status of each of `placed`, read through the hub of `node` as `subject`. All
/// reads run at once, and the call waits until [`WAIT`] after `asked`.
///
/// # Errors
///
/// [`Error::ChannelRead`] for the first of `placed` whose reader does not open or
/// ends, other than for status channels that the hub does not hold as the spec defines
/// them or that an apply removes.
pub(crate) async fn read(
    node: &Node,
    subject: &Name,
    placed: &[Placed<'_>],
    asked: Monotonic,
) -> Result<Output, Error> {
    let reads = placed.iter().map(|placed| newest(node, subject, placed));
    let readings = before(reads, node.clock.sleep_until(asked + WAIT)).await;
    let connectors = placed
        .iter()
        .zip(readings)
        .map(|(placed, reading)| Ok(status(placed, reading.transpose()?.flatten())))
        .collect::<Result<_, Error>>()?;
    Ok(Output { connectors })
}

/// The newest status of `placed`, or `None` when the frame holds no status, the hub
/// does not hold the channels of the spec yet, or an apply removed one.
///
/// # Errors
///
/// [`Error::ChannelRead`] when the reader does not open or ends for another cause.
async fn newest(
    node: &Node,
    subject: &Name,
    placed: &Placed<'_>,
) -> Result<Option<connector::status::Reading>, Error> {
    let select = Selector::new(placed.channels().map(Name::as_str));
    let select = select.expect("invariant: the names of channels select them");
    let config = Config {
        select,
        mode: Mode::Latest,
        subject: subject.clone(),
        name: None,
        hold: Span::ZERO,
    };
    let failed = |message: String| Error::ChannelRead {
        connector: placed.name.clone(),
        message,
    };
    let mut reader = match node.hub.reader(config).await {
        Ok(reader) => reader,
        Err(reader::Error::Empty | reader::Error::ManyIndexes { .. }) => {
            return Ok(None);
        }
        Err(error) => return Err(failed(error.to_string())),
    };
    match reader.next().await {
        Ok(received) if placed.defines(&received) => {
            Ok(connector::status::Reading::of(&received))
        }
        Ok(_) | Err(reader::Ended::Removed(_)) => Ok(None),
        Err(ended) => Err(failed(ended.to_string())),
    }
}

/// The output of each of `futures`, which run at once, or `None` for each that has
/// not resolved when `sleep` resolves.
async fn before<F: Future>(
    futures: impl IntoIterator<Item = F>,
    sleep: impl Future,
) -> Vec<Option<F::Output>> {
    let mut futures: Vec<_> = futures.into_iter().map(Box::pin).collect();
    let mut outputs: Vec<_> = futures.iter().map(|_| None).collect();
    let mut sleep = pin!(sleep);
    poll_fn(|cx| {
        let mut pending = false;
        for (future, output) in futures.iter_mut().zip(&mut outputs) {
            if output.is_none() {
                match future.as_mut().poll(cx) {
                    Poll::Ready(done) => *output = Some(done),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending && sleep.as_mut().poll(cx).is_pending() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    outputs
}

/// The status of `placed` from `reading`, or `unknown` with no reading.
fn status(placed: &Placed<'_>, reading: Option<connector::status::Reading>) -> Status {
    let address = placed
        .connector
        .config()
        .document()
        .attributes
        .get("address");
    let address = address.and_then(|attribute| attribute.value.kind.text());
    let mut status = Status {
        name: placed.name.to_string(),
        kind: placed.connector.kind().to_string(),
        state: State::Unknown,
        address: address.map(str::to_owned),
        restarts: None,
        counts: None,
        error: None,
    };
    if let Some(reading) = reading {
        status.state = reading.state.into();
        status.restarts = Some(reading.restarts);
        let counts = reading.counts.into_iter();
        status.counts = Some(counts.map(|(name, n)| (name.to_string(), n)).collect());
        status.error = reading.error.map(|(class, text)| Failure {
            class: class.into(),
            text,
        });
    }
    status
}

impl From<connector::status::State> for State {
    fn from(state: connector::status::State) -> Self {
        match state {
            connector::status::State::Running => Self::Running,
            connector::status::State::Restarting => Self::Restarting,
            connector::status::State::Stopped => Self::Stopped,
            connector::status::State::Ending => Self::Ending,
        }
    }
}

impl From<connector::status::Class> for Class {
    fn from(class: connector::status::Class) -> Self {
        match class {
            connector::status::Class::Config => Self::Config,
            connector::status::Class::Device => Self::Device,
            connector::status::Class::Retry => Self::Retry,
        }
    }
}

impl Output {
    /// A table of the connectors, then a line `<name>: <class> error: <text>` for
    /// each connector with an error, after a blank line. Each control character of a
    /// value is escaped, so a value stays on its line. Empty with no connector.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the table entry of `status` calls it")
    )]
    pub(crate) fn text(&self) -> String {
        if self.connectors.is_empty() {
            return String::new();
        }
        let count = |status: &Status, name: &str| {
            let value = status.counts.as_ref().and_then(|counts| counts.get(name));
            value.map_or_else(|| "-".to_owned(), ToString::to_string)
        };
        let head = ["CONNECTOR", "KIND", "STATE", "ADDRESS", "IN", "CONFIRMED"];
        let mut lines = vec![head.map(str::to_owned)];
        lines.extend(self.connectors.iter().map(|status| {
            [
                escape_controls(&status.name),
                escape_controls(&status.kind),
                status.state.word().to_owned(),
                status
                    .address
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), escape_controls),
                count(status, "in"),
                count(status, "confirmed"),
            ]
        }));
        let mut widths = [0; 6];
        for line in &lines {
            for (width, cell) in widths.iter_mut().zip(line) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let mut text = String::new();
        for line in &lines {
            let cells = line.iter().zip(widths);
            let cells: Vec<_> = cells
                .map(|(cell, width)| format!("{cell:width$}"))
                .collect();
            text.push_str(cells.join("  ").trim_end());
            text.push('\n');
        }
        let errors = self.connectors.iter().filter_map(|status| {
            let error = status.error.as_ref()?;
            let class = error.class.word();
            let (name, text) =
                (escape_controls(&status.name), escape_controls(&error.text));
            Some(format!("{name}: {class} error: {text}\n"))
        });
        let errors: String = errors.collect();
        if !errors.is_empty() {
            text.push('\n');
            text.push_str(&errors);
        }
        text
    }
}

impl State {
    const fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Restarting => "restarting",
            Self::Stopped => "stopped",
            Self::Ending => "ending",
            Self::Unknown => "unknown",
        }
    }
}

impl Class {
    const fn word(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Device => "device",
            Self::Retry => "retry",
        }
    }
}
