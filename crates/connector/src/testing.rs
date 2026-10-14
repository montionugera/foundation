//! A rig that runs connectors on a simulated hub, the inputs of a supervisor, and the
//! status channels of a connector, for the tests of `connector` and of the kinds.

use std::cell::Cell;
use std::fmt;
use std::future::poll_fn;
use std::path::Path;
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::Poll;

use document::Document;
use env::clock::Clock;
use env::files::{self, Files};
use env::net::Net;
use env::tasks::Tasks;
use hub::Hub;
use hub::reader::{Mode, Reader};
use hub::writer::Writer;
use spec::channel::{Channel, Data, Kind};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::authority::Authority;
use types::channel;
use types::name::{Name, Selector};
use types::time::Span;

use crate::cancel::Token;
use crate::kind::{Error, Table};
use crate::status::{self, Reading};
use crate::supervisor::{self, Supervisor};

#[cfg(test)]
#[cfg(not(loom))]
mod tests;

/// The first key of the status channels that [`Rig::supervise`] defines.
const STATUS: u128 = 1 << 64;

/// A hub on simulated I/O, the definitions of a test, and the supervisor of the
/// connectors that it starts, for the tests of `connector` and of each kind.
#[derive(Debug)]
pub struct Rig {
    supervisor: Supervisor,
    hub: Hub,
    kinds: Arc<Table>,
    clock: Clock,
    tasks: Tasks,
    definitions: Vec<(Name, Definition)>,
    /// The first key of the status channels of the next connector.
    next: u128,
}

impl Rig {
    /// Opens a hub on a new shard of `env`, whose connectors run the kinds of
    /// `kinds` and reach devices through `net`.
    ///
    /// # Panics
    ///
    /// As [`hub::testing::open`].
    pub async fn open(env: hub::testing::Env, net: Net, kinds: Table) -> Self {
        let config = create_config(env, net, kinds).await;
        Self {
            hub: config.hub.clone(),
            kinds: Arc::clone(&config.kinds),
            clock: config.clock.clone(),
            tasks: config.tasks.clone(),
            supervisor: Supervisor::new(config),
            definitions: Vec::new(),
            next: STATUS,
        }
    }

    /// The hub, for a reader or a write that the rig does not give. The rig does not
    /// see a call of [`Hub::set_definitions`] on it, so a later [`Self::define`] sets
    /// only the definitions that the rig holds, and the hub removes each other channel.
    #[must_use]
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// Adds `definitions` to each definition that the rig holds, and sets all of
    /// them on the hub. Each call keeps what an earlier call defined.
    ///
    /// # Panics
    ///
    /// As [`Hub::set_definitions`] on each definition that the rig holds, such as a key
    /// or a name that an earlier call used. The rig gives the status channels of each
    /// connector that it opens the next keys from 2^64 on, as [`create_status`] orders
    /// them.
    pub fn define(&mut self, definitions: Vec<(Name, Definition)>) {
        self.definitions.extend(definitions);
        let definitions = self.definitions.iter();
        self.hub
            .set_definitions(definitions.map(|(name, definition)| (name, definition)));
    }

    /// Opens a writer of `channels` as `subject`, with absolute authority and no
    /// lease.
    ///
    /// # Panics
    ///
    /// When the hub refuses the writer.
    pub async fn writer(&self, subject: Name, channels: Vec<Name>) -> Writer {
        let config = hub::writer::Config {
            subject,
            authority: Authority::ABSOLUTE,
            lease: None,
            channels,
        };
        let writer = self.hub.writer(config).await;
        writer.unwrap_or_else(|error| panic!("the writer does not open: {error}"))
    }

    /// Starts the connector `name` of `kind` with `config` in a task of the rig's
    /// shard: defines its status channels from the counts of the kind's check, then
    /// opens and runs it as [`Supervisor::open`] and [`supervisor::Opened::run`] do.
    /// When the check fails, it defines nothing and the run ends at once with
    /// [`Error::Config`].
    ///
    /// # Panics
    ///
    /// As [`kind::Table::check`](Table::check) and [`Self::define`], such as a second
    /// connector of one name, and when the status reader does not open.
    pub async fn supervise(&mut self, kind: &str, name: Name, config: Document) -> Run {
        let reader = match self.define_status(kind, &name, &config) {
            Some(select) => Some(self.status(select).await),
            None => None,
        };
        let (cancel, ended, end) =
            (Token::new(), Token::new(), Rc::new(Cell::new(None)));
        let (supervisor, kind) = (self.supervisor.clone(), kind.to_owned());
        let (token, done, into) = (cancel.clone(), ended.clone(), Rc::clone(&end));
        self.tasks.spawn(async move {
            let opened = supervisor.open(&kind, name, &config, &token).await;
            into.set(Some(opened.run().await));
            done.cancel();
        });
        Run {
            cancel,
            ended,
            end,
            reader,
            clock: self.clock.clone(),
        }
    }

    /// Defines the status channels of the connector `name` of `kind` from the counts of
    /// the kind's check, and opens it as [`Supervisor::open`] does, for a test that
    /// polls [`supervisor::Opened::run`] in its own task. When the check fails, it
    /// defines nothing, and the run ends at once with [`Error::Config`].
    ///
    /// # Panics
    ///
    /// As [`kind::Table::check`](Table::check) and [`Self::define`], such as a second
    /// connector of one name.
    pub async fn open_connector<'a>(
        &'a mut self,
        kind: &'a str,
        name: Name,
        config: &'a Document,
        cancel: &'a Token,
    ) -> supervisor::Opened<'a> {
        self.define_status(kind, &name, config);
        self.supervisor.open(kind, name, config, cancel).await
    }

    /// Defines the status channels of the connector `name` of `kind` from the counts
    /// of the kind's check, with the rig's next keys, and gives a selector of them.
    /// Gives `None`, and defines nothing, when the check fails.
    fn define_status(
        &mut self,
        kind: &str,
        name: &Name,
        config: &Document,
    ) -> Option<Selector> {
        let counts = self.kinds.check(kind, None, config).ok()?.counts;
        let first = channel::Key::from_u128(self.next);
        let status = create_status(name, &counts, first);
        let used = u128::try_from(status.len()).expect("invariant: a few channels");
        self.next += used;
        let select = Selector::new(status.iter().map(|(name, _)| name.as_str()));
        self.define(status);
        Some(select.expect("invariant: names select"))
    }

    /// Opens a reader of the status channels of `select`.
    async fn status(&self, select: Selector) -> Reader {
        let config = hub::reader::Config {
            select,
            mode: Mode::Complete,
            subject: "rig".parse().expect("invariant: a valid name"),
            name: None,
            hold: Span::ZERO,
        };
        let reader = self.hub.reader(config).await;
        reader
            .unwrap_or_else(|error| panic!("the status reader does not open: {error}"))
    }
}

/// A connector that [`Rig::supervise`] started, and a reader of its status. A drop
/// cancels the connector and does not wait for its tasks.
pub struct Run {
    cancel: Token,
    /// Cancelled once the run ended and set `end`.
    ended: Token,
    end: Rc<Cell<Option<Result<(), Error>>>>,
    /// `None` when the kind's check failed, so the connector has no status.
    reader: Option<Reader>,
    clock: Clock,
}

impl fmt::Debug for Run {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Run")
            .field("ended", &self.ended.cancelled())
            .finish_non_exhaustive()
    }
}

impl Run {
    /// Waits for the next status frame of the connector. The reader stays open after
    /// the run ends, so bound a wait that can outlive the run with [`Self::until`].
    ///
    /// # Panics
    ///
    /// When the kind's check failed, so the connector has no status, or the status
    /// reader ends.
    pub async fn next(&mut self) -> Reading {
        let reader = self.reader.as_mut();
        let reader = reader.expect("a connector whose check failed has no status");
        let received = reader.next().await;
        let received = received.unwrap_or_else(|end| panic!("the status ends: {end}"));
        let reading = Reading::of(&received);
        reading.expect("invariant: each status frame holds the whole status")
    }

    /// Waits for the first status frame for which `done` is true, and gives it.
    ///
    /// # Panics
    ///
    /// When none comes within `within` of the rig's clock, with each reading that
    /// came in the message. As [`Self::next`].
    pub async fn until(
        &mut self,
        within: Span,
        done: impl Fn(&Reading) -> bool,
    ) -> Reading {
        let clock = self.clock.clone();
        let mut late = pin!(clock.sleep(within));
        let mut readings = Vec::new();
        loop {
            let mut next = pin!(self.next());
            let reading = poll_fn(|cx| match next.as_mut().poll(cx) {
                Poll::Ready(reading) => Poll::Ready(Some(reading)),
                Poll::Pending => late.as_mut().poll(cx).map(|()| None),
            })
            .await;
            let Some(reading) = reading else {
                panic!("no status within {within} matches, after {readings:?}")
            };
            if done(&reading) {
                return reading;
            }
            readings.push(reading);
        }
    }

    /// Cancels the connector and gives the result of its run, once each task of the
    /// run ended.
    ///
    /// # Errors
    ///
    /// As [`supervisor::Opened::run`].
    pub async fn stop(self) -> Result<(), Error> {
        self.cancel.cancel();
        self.ended.wait().await;
        let Some(end) = self.end.take() else {
            unreachable!("the run sets its end before `ended`")
        };
        end
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The directory of the files that holds the secrets of [`create_secret_store`].
const SECRETS: &str = "secrets";

/// A supervisor's inputs for the tests of `connector` and the kinds: `kinds`, the
/// seams of `env` and `net`, a hub on a new shard in `env`, and the store of
/// [`create_secret_store`] on `env.files`.
///
/// # Panics
///
/// As [`hub::testing::open`].
pub async fn create_config(
    env: hub::testing::Env,
    net: Net,
    kinds: Table,
) -> supervisor::Config {
    let (clock, entropy, tasks) =
        (env.clock.clone(), env.entropy.clone(), env.tasks.clone());
    let files = env.files.clone();
    let (hub, _) = hub::testing::open(env).await;
    supervisor::Config {
        kinds: Arc::new(kinds),
        clock,
        entropy,
        net,
        tasks,
        hub,
        secrets: create_secret_store(files),
    }
}

/// A store of the secrets on `files` that [`create_secret`] writes.
#[must_use]
pub fn create_secret_store(files: Files) -> Box<dyn secret::store::Store> {
    Box::new(secret::store::File::new(files, Path::new(SECRETS)))
}

/// Writes `value` as the secret `name` in the store that [`create_secret_store`]
/// makes on `files`, in place of a value that it had.
///
/// # Panics
///
/// When a call of `files` fails.
pub async fn create_secret(files: &Files, name: &Name, value: &[u8]) {
    let dir = Path::new(SECRETS);
    files.create_dir(dir).await.expect("makes the directory");
    let path = dir.join(name.as_str());
    files.remove(&path).await.expect("removes the old value");
    let len = u64::try_from(value.len()).expect("invariant: usize fits u64");
    let file = files.open(&path, files::Mode::Create { len }).await;
    let file = file.expect("makes the file");
    let pool = block::Config::new(1 << 18).expect("a budget that fits");
    let block = block::Pool::heap(pool).copy(value);
    let block = block.expect("the pool holds the value");
    file.write_at(0, &[block]).await.expect("writes the value");
    file.close().await;
}

/// The definitions of the status channels of `connector`, whose kind names `counts`,
/// with keys from `first` on, the index first. A test passes them to [`Rig::define`],
/// or to [`Hub::set_definitions`] with its own definitions when it replaces the whole
/// set.
///
/// # Panics
///
/// When [`status::channels`] gives an error.
#[must_use]
pub fn create_status(
    connector: &Name,
    counts: &[Name],
    first: channel::Key,
) -> Vec<(Name, Definition)> {
    let names = status::channels(connector, counts);
    let (time, channels) = names.expect("the status names are names");
    let index = first;
    let kind = Kind::Index {
        error: None,
        control: None,
    };
    let time = (time, Definition::Channel(Channel { key: index, kind }));
    let channels =
        (first.as_u128() + 1..)
            .zip(channels)
            .map(|(key, (name, sample))| {
                let data = Data::new(index, None, DataType::Sample(sample), None);
                let data = data.expect("invariant: a status channel has no unit");
                let key = channel::Key::from_u128(key);
                let kind = Kind::Data(data);
                (name, Definition::Channel(Channel { key, kind }))
            });
    std::iter::once(time).chain(channels).collect()
}
