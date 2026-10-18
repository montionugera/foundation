//! A rig that runs connectors on a simulated hub, the store of their secrets, the
//! status channels of a connector, and a capture of events, for the tests of
//! `connector` and of the kinds.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::future::poll_fn;
use std::path::Path;
use std::pin::pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use document::Document;
use env::clock::Clock;
use env::files::{self, Files};
use env::net::Net;
use env::tasks::Tasks;
use hub::Hub;
use hub::reader::{Mode, Reader, Received};
use hub::writer::Writer;
use spec::channel::{Channel, Data, Kind};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::authority::Authority;
use types::channel;
use types::name::{Name, Selector};
use types::time::Span;

use crate::cancel::Token;
use crate::http::Roots;
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
    definitions: RefCell<Vec<(Name, Definition)>>,
    /// The status channels that the rig defined, by connector.
    held: RefCell<BTreeMap<Name, Held>>,
    /// The first key of the status channels of the next connector.
    next: Cell<u128>,
}

impl Rig {
    /// Opens a hub on a new shard of `env`, whose connectors run the kinds of
    /// `kinds`, reach devices through `net`, and read secrets from the store of
    /// [`create_secrets`] on `env.files`.
    ///
    /// # Panics
    ///
    /// As [`hub::testing::open`].
    pub async fn open(env: hub::testing::Env, net: Net, kinds: Table) -> Self {
        Self::trusting(env, net, kinds, Roots::default()).await
    }

    /// Opens a rig as [`Self::open`] does, whose connectors trust `roots` for an
    /// `https` connect.
    ///
    /// # Panics
    ///
    /// As [`hub::testing::open`].
    pub async fn trusting(
        env: hub::testing::Env,
        net: Net,
        kinds: Table,
        roots: Roots,
    ) -> Self {
        let config = supervisor::Config {
            roots,
            ..create_config(env, net, kinds).await
        };
        Self::new(config)
    }

    /// Opens a rig as [`Self::open`] does, on a hub whose mesh time starts `delay`
    /// after the call, so the node has no mesh time until then.
    ///
    /// # Panics
    ///
    /// As [`hub::testing::open_unsynced`].
    pub async fn unsynced(
        env: hub::testing::Env,
        net: Net,
        kinds: Table,
        delay: Span,
    ) -> Self {
        let open = async move |env| hub::testing::open_unsynced(env, delay).await;
        Self::new(create_config_on(env, net, kinds, open).await)
    }

    fn new(config: supervisor::Config) -> Self {
        Self {
            hub: config.hub.clone(),
            kinds: Arc::clone(&config.kinds),
            clock: config.clock.clone(),
            tasks: config.tasks.clone(),
            supervisor: Supervisor::new(config),
            definitions: RefCell::default(),
            held: RefCell::default(),
            next: Cell::new(STATUS),
        }
    }

    /// The hub, for a reader or a write that the rig does not give. The rig does not
    /// see a call of [`Hub::set_definitions`] on it, so a later [`Self::define`] or
    /// [`Self::remove`] sets only the definitions that the rig holds, and the hub
    /// removes each other channel.
    #[must_use]
    pub fn hub(&self) -> &Hub {
        &self.hub
    }

    /// The supervisor of the rig's connectors, for a browse.
    #[must_use]
    pub fn supervisor(&self) -> &Supervisor {
        &self.supervisor
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
    pub fn define(&self, definitions: Vec<(Name, Definition)>) {
        self.definitions.borrow_mut().extend(definitions);
        self.set();
    }

    /// Removes `names` from the definitions that the rig holds, and sets the rest on
    /// the hub, so the hub drops the channels of `names`. A name that the rig does not
    /// hold is not an error. The rig keeps the status names of each connector name
    /// after a remove, so a later open of that name panics when a status channel is
    /// gone, unless its cancel is cancelled.
    ///
    /// # Panics
    ///
    /// As [`Self::define`].
    pub fn remove(&self, names: &[Name]) {
        let mut definitions = self.definitions.borrow_mut();
        definitions.retain(|(name, _)| !names.contains(name));
        drop(definitions);
        self.set();
    }

    /// Sets each definition that the rig holds on the hub.
    fn set(&self) {
        let definitions = self.definitions.borrow();
        let definitions = definitions.iter();
        self.hub
            .set_definitions(definitions.map(|(name, definition)| (name, definition)));
    }

    /// Opens a writer of `channels` as `subject`, with absolute authority and no limit
    /// on its lease.
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
    /// shard: defines its status channels as [`Self::define_status`] does, then opens
    /// and runs it as [`Supervisor::open`] and [`supervisor::Opened::run`] do. When the
    /// check fails, it defines nothing and the run ends at once with
    /// [`Error::Config`].
    ///
    /// # Panics
    ///
    /// As [`Self::define_status`] but at a failed check, and when the status reader
    /// does not open.
    pub async fn supervise(&self, kind: &str, name: Name, config: Document) -> Run {
        let (cancel, ended, end) =
            (Token::new(), Token::new(), Rc::new(Cell::new(None)));
        let reader = match self.kinds.check(kind, None, &config) {
            Ok(channels) => {
                let status = self.hold(kind, &name, channels.counts);
                self.track(&name, ended.clone());
                Some(self.status(&status).await)
            }
            Err(_) => None,
        };
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
            pending: VecDeque::new(),
            clock: self.clock.clone(),
        }
    }

    /// Defines the status channels of the connector `name` of `kind` as
    /// [`Self::define_status`] does, and opens it as [`Supervisor::open`] does, for a
    /// test that polls [`supervisor::Opened::run`] in its own task. When the check
    /// fails, it defines nothing, and the run ends at once with [`Error::Config`].
    ///
    /// # Panics
    ///
    /// As [`Self::define_status`] but at a failed check.
    pub async fn open_connector<'a>(
        &'a self,
        kind: &'a str,
        name: Name,
        config: &'a Document,
        cancel: &'a Token,
    ) -> supervisor::Opened<'a> {
        if let Ok(channels) = self.kinds.check(kind, None, config) {
            self.hold(kind, &name, channels.counts);
        }
        self.supervisor.open(kind, name, config, cancel).await
    }

    /// Defines the status channels of the connector `name` of `kind` from the counts
    /// of the kind's check, with the rig's next keys, and gives their names, the
    /// index first. When the rig holds them for `name`, it defines nothing and gives
    /// the same names, so a reader of them goes on. [`Self::supervise`] and
    /// [`Self::open_connector`] define the same channels, and define nothing when
    /// the check fails. Call it first to open a reader of the status before the
    /// connector starts.
    ///
    /// # Panics
    ///
    /// When the kind's check fails, when the rig holds the status channels of `name`
    /// for other counts, while a run of `name` that [`Self::supervise`] started goes
    /// on (a dropped [`Run`] goes on until its tasks end), and as [`Self::define`].
    pub fn define_status(
        &self,
        kind: &str,
        name: &Name,
        config: &Document,
    ) -> Vec<Name> {
        let checked = self.kinds.check(kind, None, config);
        let checked = checked.unwrap_or_else(|diagnostics| {
            panic!("the check of {kind} fails: {diagnostics:?}")
        });
        self.hold(kind, name, checked.counts)
    }

    /// As [`Self::define_status`], from the `counts` of a check that passed.
    fn hold(&self, kind: &str, name: &Name, counts: Vec<Name>) -> Vec<Name> {
        if let Some(held) = self.held.borrow().get(name) {
            assert!(
                held.counts == counts,
                "the rig holds the status of {name} with the counts [{}], and the \
                 check of {kind} gives [{}]",
                join(&held.counts),
                join(&counts),
            );
            assert!(
                held.run.as_ref().is_none_or(Token::cancelled),
                "a run of {name} goes on: stop it before the next open of its name"
            );
            return held.names.clone();
        }
        let first = self.next.get();
        let status = create_status(name, &counts, channel::Key::from_u128(first));
        let names: Vec<_> = status.iter().map(|(name, _)| name.clone()).collect();
        let used = u128::try_from(status.len()).expect("invariant: a few channels");
        self.next.set(first + used);
        self.define(status);
        let held = Held {
            counts,
            names: names.clone(),
            run: None,
        };
        self.held.borrow_mut().insert(name.clone(), held);
        names
    }

    /// Makes each next open of `name` panic until `run` is cancelled. The rig holds
    /// the status channels of `name`.
    fn track(&self, name: &Name, run: Token) {
        let mut held = self.held.borrow_mut();
        held.get_mut(name)
            .expect("invariant: the rig holds the name")
            .run = Some(run);
    }

    /// Records each status of the connector whose status channels are `names`, as
    /// [`Self::define_status`] gives them, from now on, in a task of the rig's shard.
    /// The record goes on across the runs of the connector.
    ///
    /// # Panics
    ///
    /// When the status reader does not open, such as when the hub defines no channel
    /// of `names`. The task of the record panics, which ends the shard, as
    /// [`Run::next`] does: when the status reader ends, misses a status, or reads a
    /// frame that does not hold a whole status.
    pub async fn record(&self, names: &[Name]) -> Record {
        let mut reader = self.status(names).await;
        let record = Record(Rc::default());
        let into = Rc::clone(&record.0);
        self.tasks.spawn(async move {
            loop {
                let received = reader.next().await;
                let received =
                    received.unwrap_or_else(|end| panic!("the status ends: {end}"));
                into.borrow_mut().extend(readings(&received));
            }
        });
        record
    }

    /// Opens a reader of the status channels `names`.
    async fn status(&self, names: &[Name]) -> Reader {
        let select = Selector::new(names.iter().map(Name::as_str));
        let config = hub::reader::Config {
            select: select.expect("invariant: names select"),
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

/// The status channels that the rig defined for one connector.
#[derive(Debug)]
struct Held {
    counts: Vec<Name>,
    /// The names, the index first.
    names: Vec<Name>,
    /// Cancelled once the last run that [`Rig::supervise`] started for the connector
    /// ends. `None` when it started none.
    run: Option<Token>,
}

/// Each status of one connector that [`Rig::record`] read, one for each sample time
/// of its status channels.
#[derive(Clone, Debug)]
pub struct Record(Rc<RefCell<Vec<Reading>>>);

impl Record {
    /// Each status so far, oldest first.
    #[must_use]
    pub fn readings(&self) -> Vec<Reading> {
        self.0.borrow().clone()
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
    /// The statuses of the last frame that [`Self::next`] did not give yet.
    pending: VecDeque<Reading>,
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
    /// Cancels the connector as [`Self::stop`] does, and gives the result of its run
    /// only when the run ended before the call, so that a test can tell the end of the
    /// run from the end of the cancel. The `Err` inside is as
    /// [`supervisor::Opened::run`] gives it.
    pub async fn end(self) -> Option<Result<(), Error>> {
        let ended = self.ended.cancelled();
        let end = self.stop().await;
        ended.then_some(end)
    }

    /// Waits for the next status of the connector, oldest first: a frame that holds
    /// more than one status gives each in turn. The reader stays open after the run
    /// ends, so bound a wait that can outlive the run with [`Self::until`].
    ///
    /// # Panics
    ///
    /// When the kind's check failed, so the connector has no status, or the status
    /// reader ends, misses a status, or reads a frame that does not hold a whole status.
    pub async fn next(&mut self) -> Reading {
        loop {
            if let Some(reading) = self.pending.pop_front() {
                return reading;
            }
            let reader = self.reader.as_mut();
            let reader = reader.expect("a connector whose check failed has no status");
            let received = reader.next().await;
            let received =
                received.unwrap_or_else(|end| panic!("the status ends: {end}"));
            self.pending = readings(&received).into();
        }
    }

    /// Waits for the first status for which `done` is true, and gives it.
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

/// Each status of the status frame `received`, oldest first.
///
/// # Panics
///
/// When the reader missed a status before `received`, or `received` does not hold a
/// whole status.
fn readings(received: &Received<'_>) -> Vec<Reading> {
    if let Some(gap) = received.gap() {
        panic!(
            "the status reader missed the seqs {}..{}",
            gap.start, gap.end
        );
    }
    Reading::each(received).unwrap_or_else(|| {
        let view = received.view();
        let names: Vec<_> =
            view.iter().map(|(entry, _)| received.name(entry)).collect();
        panic!(
            "a status frame of [{}] holds no whole status",
            names.join(", ")
        )
    })
}

/// `names` with `, ` between them.
fn join(names: &[Name]) -> String {
    let names: Vec<_> = names.iter().map(Name::as_str).collect();
    names.join(", ")
}

/// The directory of the files that holds the secrets of [`create_secrets`].
const SECRETS: &str = "secrets";

/// A supervisor's inputs for the rig and the unit tests of `connector`: `kinds`, the
/// seams of `env` and `net`, a hub on a new shard in `env`, the store of
/// [`create_secrets`] on `env.files`, and no roots.
///
/// # Panics
///
/// As [`hub::testing::open`].
pub(crate) async fn create_config(
    env: hub::testing::Env,
    net: Net,
    kinds: Table,
) -> supervisor::Config {
    create_config_on(env, net, kinds, async |env| hub::testing::open(env).await.0).await
}

/// A config as [`create_config`] gives, on the hub that `open` opens on `env`.
async fn create_config_on(
    env: hub::testing::Env,
    net: Net,
    kinds: Table,
    open: impl AsyncFnOnce(hub::testing::Env) -> Hub,
) -> supervisor::Config {
    let (clock, entropy, tasks) =
        (env.clock.clone(), env.entropy.clone(), env.tasks.clone());
    let files = env.files.clone();
    let hub = open(env).await;
    supervisor::Config {
        kinds: Arc::new(kinds),
        clock,
        entropy,
        net,
        roots: Roots::default(),
        tasks,
        hub,
        secrets: create_secrets(files),
    }
}

/// A resolver with no environment variables, whose store holds the secrets on
/// `files` that [`create_secret`] writes.
#[must_use]
pub fn create_secrets(files: Files) -> secret::Resolver {
    secret::Resolver::new(Arc::default(), files, Path::new(SECRETS))
}

/// Writes `value` as the secret `name` in the store that [`create_secrets`] makes
/// on `files`, in place of a value that it had. A read of the store meanwhile gives
/// the old value, no value, or `value`, never a part of one.
///
/// # Panics
///
/// When a call of `files` fails, such as when another call for `name` runs at once.
pub async fn create_secret(files: &Files, name: &Name, value: &[u8]) {
    let dir = Path::new(SECRETS);
    files.create_dir(dir).await.expect("makes the directory");
    let len = u64::try_from(value.len()).expect("invariant: usize fits u64");
    // No name starts with `.`, so the store never reads a staged file. Each call
    // takes the first slot that no other call holds and no dropped call left at
    // another length.
    let mut slot = 0_u32;
    let mut file = loop {
        let staged = dir.join(format!(".{slot}"));
        match files.open(&staged, files::Mode::Create { len }).await {
            Ok(file) => break file,
            Err(files::Error::Busy { .. } | files::Error::Length { .. }) => slot += 1,
            Err(error) => panic!("makes the file: {error:?}"),
        }
    };
    let pool = block::Config::new(1 << 18).expect("a budget that fits");
    let block = block::Pool::heap(pool).copy(value);
    let block = block.expect("the pool holds the value");
    file.write_at(0, &[block]).await.expect("writes the value");
    let path = dir.join(name.as_str());
    files.remove(&path).await.expect("removes the old value");
    file.rename(&path).await.expect("puts the value in place");
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

/// Takes the events of one target from the dispatch that [`Capture::new`] gives, each
/// as one line `<LEVEL> <message> <key>=<value>...`, with the fields of the event's
/// explicit parent span first and each value in `Debug`.
#[derive(Debug)]
pub struct Capture(Arc<Captured>);

#[derive(Debug)]
struct Captured {
    target: &'static str,
    /// The fields of span `n` at index `n - 1`, and the lines.
    log: Mutex<(Vec<String>, Vec<String>)>,
    /// A second live dispatch. With one, `tracing` caches the interest of a callsite
    /// from the default of the thread that first reaches it, so a test with no
    /// dispatch on another thread turns that callsite off for each capture.
    _other: tracing::Dispatch,
}

impl Capture {
    /// A capture of the events of `target`, and the dispatch that feeds it.
    #[must_use]
    pub fn new(target: &'static str) -> (Self, tracing::Dispatch) {
        let log = Mutex::new((Vec::new(), Vec::new()));
        let capture = Self(Arc::new(Captured {
            target,
            log,
            _other: tracing::Dispatch::new(tracing::subscriber::NoSubscriber::new()),
        }));
        let dispatch = tracing::Dispatch::new(Arc::clone(&capture.0));
        (capture, dispatch)
    }

    /// The lines so far, in event order.
    ///
    /// # Panics
    ///
    /// When a panic held the lock of the lines.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        self.0
            .log
            .lock()
            .expect("no panic holds the lock")
            .1
            .clone()
    }
}

impl tracing::Subscriber for Captured {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let fields = fields(|visit| span.record(visit));
        let mut log = self.log.lock().expect("no panic holds the lock");
        log.0.push(fields);
        let id = u64::try_from(log.0.len()).expect("invariant: a span count fits u64");
        tracing::span::Id::from_u64(id)
    }

    fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let more = fields(|visit| values.record(visit));
        let mut log = self.log.lock().expect("no panic holds the lock");
        let fields = &mut log.0[index(span)];
        if !more.is_empty() {
            if !fields.is_empty() {
                fields.push(' ');
            }
            fields.push_str(&more);
        }
    }

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let metadata = event.metadata();
        if metadata.target() != self.target {
            return;
        }
        let mut message = String::new();
        let mut rest = Vec::new();
        event.record(
            &mut |field: &tracing::field::Field, value: &dyn fmt::Debug| {
                if field.name() == "message" {
                    message = format!("{value:?}");
                } else {
                    rest.push(format!("{}={value:?}", field.name()));
                }
            },
        );
        let mut log = self.log.lock().expect("no panic holds the lock");
        let mut parts = vec![metadata.level().to_string(), message];
        if let Some(parent) = event.parent() {
            parts.extend(Some(log.0[index(parent)].clone()).filter(|f| !f.is_empty()));
        }
        parts.extend(rest);
        log.1.push(parts.join(" "));
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// The fields that `record` gives, as `<key>=<value>...`, values in `Debug`.
fn fields(record: impl FnOnce(&mut dyn tracing::field::Visit)) -> String {
    let mut fields = Vec::new();
    record(
        &mut |field: &tracing::field::Field, value: &dyn fmt::Debug| {
            fields.push(format!("{}={value:?}", field.name()));
        },
    );
    fields.join(" ")
}

/// The index of the fields of `span` in the log of a [`Capture`].
fn index(span: &tracing::span::Id) -> usize {
    let id = usize::try_from(span.into_u64()).expect("invariant: a span id fits usize");
    id - 1
}
