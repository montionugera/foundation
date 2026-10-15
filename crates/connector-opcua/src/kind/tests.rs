#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use ::sim::{Sim, node};
use connector::cancel::Token;
use connector::kind::Table;
use connector::supervisor::Supervisor;
use connector::testing;
use document::value::{self, Value};
use document::{Attribute, Block, Map};
use env::net::{Listener, tcp};
use env::tasks::{Group, Tasks};
use hub::reader::{Mode, Received};
use spec::channel::{Channel, Data};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::channel;
use types::name::{Name, Selector};
use types::quality::Quality;

use super::*;
use crate::sim::{Served, Variables, serve};

const A: &str = "ns=3;s=SpikeData";
const B: &str = "ns=3;i=7";
const C: &str = "ns=2;s=Dip";

/// The data channels of [`Setup::three`].
const DATA: &[&str] = &["plant.a", "plant.b", "plant.c"];

/// The connector of each scene.
const PLC: &str = "plant.plc";

/// The first key of the status channels of [`PLC`]: the index, then `state`,
/// `class`, `restarts`, `backoff`, `error`, `note`, then each of [`COUNTS`].
const STATUS: u128 = 100;
const CLASS: u128 = STATUS + 2;
const RESTARTS: u128 = STATUS + 3;
const ERROR: u128 = STATUS + 5;
const NOTE: u128 = STATUS + 6;
const IN: u128 = STATUS + 7;
const MISSED: u128 = STATUS + 8;
const LOST: u128 = STATUS + 9;
const BAD: u128 = STATUS + 10;
const UNCERTAIN: u128 = STATUS + 11;
const MISTYPED: u128 = STATUS + 12;

/// The class of a device error in the status.
const DEVICE: u64 = 2;

fn ms(n: i64) -> Span {
    Span::from_nanos(n * 1_000_000)
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// The samples of one series that a reader received.
#[derive(Clone, Debug, PartialEq)]
enum Samples {
    /// Each sample of a fixed width, widened to 8 bytes.
    Fixed(Vec<u64>),
    Texts(Vec<String>),
}

/// The series of one frame that a reader received, by key.
type Frame = BTreeMap<u128, Samples>;

/// The samples of `key` in `frame`, when it holds them.
fn fixed(frame: &Frame, key: u128) -> Option<&[u64]> {
    match frame.get(&key)? {
        Samples::Fixed(samples) => Some(samples),
        Samples::Texts(_) => panic!("{key} has fixed samples"),
    }
}

/// The value of the one sample of the `f64` channel `key` in `frame`.
fn value(frame: &Frame, key: u128) -> f64 {
    match fixed(frame, key) {
        Some(&[bits]) => f64::from_bits(bits),
        other => panic!("one sample of {key}: {other:?}"),
    }
}

/// The stamp of the one sample of the index `key` in `frame`.
fn stamp(frame: &Frame, key: u128) -> i64 {
    match fixed(frame, key) {
        Some(&[bits]) => i64::from_le_bytes(bits.to_le_bytes()),
        other => panic!("one stamp of {key}: {other:?}"),
    }
}

/// The last sample of the status channel `key` in `statuses`, or 0 when none holds
/// it.
fn last(statuses: &[Frame], key: u128) -> u64 {
    let mut samples = statuses.iter().filter_map(|frame| fixed(frame, key));
    samples
        .next_back()
        .and_then(|samples| samples.last().copied())
        .unwrap_or(0)
}

/// Each text of `error` in `statuses` that is not empty, with the class beside it,
/// once for each run of equal pairs.
fn errors(statuses: &[Frame]) -> Vec<(u64, String)> {
    let mut errors = Vec::new();
    for frame in statuses {
        let (Some(Samples::Texts(texts)), Some(classes)) =
            (frame.get(&ERROR), fixed(frame, CLASS))
        else {
            continue;
        };
        let pairs = classes.iter().zip(texts);
        let pairs = pairs.filter(|(_, text)| !text.is_empty());
        errors.extend(pairs.map(|(&class, text)| (class, text.clone())));
    }
    errors.dedup();
    errors
}

/// Each text of `note` in `statuses` that is not empty, once for each run of equal
/// texts.
fn notes(statuses: &[Frame]) -> Vec<String> {
    let mut notes = Vec::new();
    for frame in statuses {
        if let Some(Samples::Texts(texts)) = frame.get(&NOTE) {
            notes.extend(texts.iter().filter(|text| !text.is_empty()).cloned());
        }
    }
    notes.dedup();
    notes
}

/// The keys of each of `frames`.
fn keys(frames: &[Frame]) -> Vec<Vec<u128>> {
    frames
        .iter()
        .map(|frame| frame.keys().copied().collect())
        .collect()
}

/// The series of `received`, by key.
fn decode(received: &Received<'_>) -> Frame {
    let (set, view) = (received.set(), received.view());
    let mut frame = Frame::new();
    for (entry, bytes) in view.iter() {
        let entry = &set.entries()[entry];
        let range = view.range(entry.group).expect("the group is present");
        let count = usize::try_from(range.count).expect("a count");
        let mut raw = Vec::new();
        codec::decode(entry.data_type, count, bytes, &mut raw).expect("decodes");
        let samples = if let Some(width) = entry.data_type.width() {
            let widen = |chunk: &[u8]| {
                let mut sample = [0; 8];
                sample[..width].copy_from_slice(chunk);
                u64::from_le_bytes(sample)
            };
            Samples::Fixed(raw.chunks(width).map(widen).collect())
        } else {
            let variable = codec::Variable::of(entry.data_type).expect("variable");
            let texts = variable.samples(count, &raw);
            let texts = texts.map(|text| String::from_utf8_lossy(text).into_owned());
            Samples::Texts(texts.collect())
        };
        frame.insert(entry.key.as_u128(), samples);
    }
    frame
}

/// Reads each frame of `names` into the vector it gives, in a task on `tasks`. A
/// frame that the home loses ends the reader `Behind`, so the task opens it again.
async fn record(
    hub: &hub::Hub,
    names: &[Name],
    tasks: &Tasks,
) -> Rc<RefCell<Vec<Frame>>> {
    let config = hub::reader::Config {
        select: Selector::new(names.iter().map(Name::as_str)).expect("a selector"),
        mode: Mode::Complete,
        subject: name("plant.test"),
        name: None,
        hold: Span::ZERO,
    };
    let mut reader = hub.reader(config.clone()).await.expect("the reader opens");
    let frames = Rc::new(RefCell::new(Vec::new()));
    let into = Rc::clone(&frames);
    let hub = hub.clone();
    tasks.spawn(async move {
        loop {
            match reader.next().await {
                Ok(received) => into.borrow_mut().push(decode(&received)),
                Err(hub::reader::Ended::Behind) => {
                    let again = hub.reader(config.clone()).await;
                    reader = again.expect("the reader opens again");
                }
                Err(_) => break,
            }
        }
    });
    frames
}

/// The definition of an index of `key`.
fn index(key: u128) -> Definition {
    Definition::Channel(Channel {
        key: channel::Key::from_u128(key),
        kind: spec::channel::Kind::Index {
            error: None,
            control: None,
        },
    })
}

/// The definition of a channel of `key` with samples of `sample`, on the index of
/// `index`.
fn data(key: u128, index: u128, sample: Type) -> Definition {
    let index = channel::Key::from_u128(index);
    let data = Data::new(index, None, DataType::Sample(sample), None);
    Definition::Channel(Channel {
        key: channel::Key::from_u128(key),
        kind: spec::channel::Kind::Data(data.expect("no unit")),
    })
}

const F64: Type = Type::Scalar(Scalar::F64);

/// The key of the index of the channel of `definition`.
fn index_of(definition: &Definition) -> u128 {
    let Definition::Channel(channel) = definition else {
        panic!("a channel: {definition:?}");
    };
    match &channel.kind {
        spec::channel::Kind::Index { .. } => channel.key.as_u128(),
        spec::channel::Kind::Data(data) => data.index().as_u128(),
    }
}

/// What a scene serves, defines, and reads.
struct Setup {
    /// The variables of the server.
    nodes: Vec<String>,
    /// The channels other than the status channels, each with its definition.
    channels: Vec<(&'static str, Definition)>,
    /// The channel and the node of each `read` block.
    reads: Vec<(&'static str, String)>,
}

impl Setup {
    /// The variables of [`Setup::nodes`].
    fn variables(&self) -> Variables {
        let nodes: Vec<_> = self.nodes.iter().map(String::as_str).collect();
        Variables::new(&nodes)
    }

    /// `plant.a`, `plant.b`, and `plant.c` (keys 2 to 4) on `plant.time` (key 1),
    /// which read [`A`], [`B`], and [`C`].
    fn three() -> Self {
        Self {
            nodes: [A, B, C].map(String::from).into(),
            channels: vec![
                ("plant.time", index(1)),
                ("plant.a", data(2, 1, F64)),
                ("plant.b", data(3, 1, F64)),
                ("plant.c", data(4, 1, F64)),
            ],
            reads: vec![
                ("plant.a", A.into()),
                ("plant.b", B.into()),
                ("plant.c", C.into()),
            ],
        }
    }
}

/// An attribute of `key` and `kind`, with no span.
fn attribute(key: &str, kind: value::Kind) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value: Value { kind, span: None },
    }
}

/// The config of a connector of `address` at 10 Hz with `reads`.
fn config(address: &str, reads: &[(&str, String)]) -> Document {
    let text = |text: &str| value::Kind::String(text.into());
    let attributes = vec![
        attribute("address", text(address)),
        attribute("rate", text("10Hz")),
    ];
    let blocks = reads.iter().map(|(channel, node)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![document::Label {
            text: (*channel).into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![attribute("node_id", text(node))])
                .expect("one key"),
            blocks: Vec::new(),
        },
        span: None,
    });
    Document {
        attributes: Map::new(attributes).expect("unique keys"),
        blocks: blocks.collect(),
    }
}

/// Binds a listener of `node` on its port 4840.
fn listen(node: &node::Node) -> Listener {
    let local = SocketAddr::new(node.addresses()[0], 4840);
    let listen = tcp::Listen {
        local,
        backlog: 4,
        options: tcp::Options {
            send_buffer_bytes: 1 << 16,
            recv_buffer_bytes: 1 << 16,
            unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
            delayed: false,
        },
    };
    node.net().listen(&listen).expect("the port is free")
}

/// One node with a hub, a server of [`Setup::nodes`] on its port 4840, and a
/// supervisor of the `opcua` kind.
struct Scene {
    node: node::Node,
    tasks: Tasks,
    hub: hub::Hub,
    supervisor: Supervisor,
    variables: Variables,
    config: Document,
    /// The stop of the server that runs, and the group of its task.
    server: RefCell<Option<(Token, Group)>>,
}

impl Scene {
    /// Starts a server of the variables on port 4840.
    fn serve(&self) {
        let listener = listen(&self.node);
        let (stop, group) = (Token::new(), Group::new(self.tasks.clone()));
        let (clock, net, node) = (self.node.clock(), self.node.net(), self.node.wall());
        #[expect(
            clippy::disallowed_methods,
            reason = "a simulated device reads the wall time of its node"
        )]
        let wall = move || node.now().time;
        let (variables, until) = (self.variables.clone(), stop.clone());
        let mut rng = self.node.entropy().rng();
        group.tasks().spawn(async move {
            let until = until.wait();
            serve(clock, wall, net, listener, &mut rng, &variables, until).await;
        });
        *self.server.borrow_mut() = Some((stop, group));
    }

    /// Stops the server, and waits until it closed.
    async fn stop(&self) {
        let server = self.server.borrow_mut().take();
        let (stop, group) = server.expect("a server runs");
        stop.cancel();
        group.ended().await;
    }

    /// Supervises [`PLC`] for `span`, then cancels it, and gives what the call
    /// returns.
    async fn supervise(&self, span: Span) -> Result<(), Error> {
        let (token, clock) = (Token::new(), self.node.clock());
        let (canceller, sleeper) = (token.clone(), clock.clone());
        self.tasks.spawn(async move {
            sleeper.sleep(span).await;
            canceller.cancel();
        });
        let open = self
            .supervisor
            .open("opcua", name(PLC), &self.config, &token);
        open.await.run().await
    }

    /// Runs `step` in a task after `after`.
    fn after<F>(
        self: &Rc<Self>,
        after: Span,
        step: impl FnOnce(Rc<Self>) -> F + 'static,
    ) where
        F: Future<Output = ()> + 'static,
    {
        let scene = Rc::clone(self);
        self.tasks.spawn(async move {
            scene.node.clock().sleep(after).await;
            step(scene).await;
        });
    }
}

/// What a scene recorded.
struct Record {
    /// What the script returned.
    result: Result<(), Error>,
    /// The frames of the channels of the setup on the index of key 1.
    frames: Vec<Frame>,
    /// The frames of the channels of the setup on each other index, by its key.
    indexes: BTreeMap<u128, Vec<Frame>>,
    /// The frames of the status channels.
    statuses: Vec<Frame>,
    served: BTreeMap<String, Vec<Served>>,
}

/// Runs `script` on a scene of `setup`, with its server started, and records the
/// frames of each channel until 1 s after the script returns.
fn run<S, F>(setup: Setup, script: S) -> Record
where
    S: FnOnce(Rc<Scene>) -> F + Send + 'static,
    F: Future<Output = Result<(), Error>> + 'static,
{
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let kinds = Table::new().with("opcua", Kind::default());
        let inputs = testing::create_config(env, node.net(), kinds).await;
        let hub = inputs.hub.clone();
        let counts = COUNTS.map(name);
        let status = testing::create_status(
            &name(PLC),
            &counts,
            channel::Key::from_u128(STATUS),
        );
        let channels = setup.channels.iter();
        let channels: Vec<_> = channels
            .map(|(text, def)| (name(text), def.clone()))
            .collect();
        let definitions = status.iter().chain(&channels);
        hub.set_definitions(definitions.map(|(name, def)| (name, def)));
        let mut groups = BTreeMap::<u128, Vec<Name>>::new();
        for (name, definition) in &channels {
            groups
                .entry(index_of(definition))
                .or_default()
                .push(name.clone());
        }
        let mut indexes = BTreeMap::new();
        for (index, names) in groups {
            indexes.insert(index, record(&hub, &names, &tasks).await);
        }
        let names: Vec<_> = status.iter().map(|(name, _)| name.clone()).collect();
        let statuses = record(&hub, &names, &tasks).await;
        let address = format!("opc.tcp://{}:4840", node.addresses()[0]);
        let scene = Rc::new(Scene {
            config: config(&address, &setup.reads),
            variables: setup.variables(),
            supervisor: Supervisor::new(inputs),
            server: RefCell::new(None),
            node: node.clone(),
            tasks,
            hub,
        });
        scene.serve();
        let result = script(Rc::clone(&scene)).await;
        if scene.server.borrow().is_some() {
            scene.stop().await;
        }
        node.clock().sleep(Span::SECOND).await;
        let indexes = indexes
            .into_iter()
            .map(|(index, frames)| (index, frames.take()));
        let indexes: BTreeMap<u128, Vec<Frame>> = indexes.collect();
        let statuses = statuses.take();
        let served = scene.variables.served();
        (result, indexes, statuses, served)
    });
    let (result, mut indexes, statuses, served) = run.expect("the run ends");
    Record {
        result,
        frames: indexes.remove(&1).expect("the index 1"),
        indexes,
        statuses,
        served,
    }
}

/// The values that the server served for `node`.
fn served(outcome: &Record, node: &str) -> Vec<f64> {
    outcome.served[node]
        .iter()
        .map(|served| served.value)
        .collect()
}

/// The values of the `f64` channel `key` in each frame.
fn values(frames: &[Frame], key: u128) -> Vec<f64> {
    frames.iter().map(|frame| value(frame, key)).collect()
}

#[test]
fn writes_the_value_that_the_server_served_for_each_node_in_one_frame_per_tick() {
    let outcome = run(Setup::three(), |scene| async move {
        for (node, value) in [(A, 10.0), (B, 20.0), (C, 30.0)] {
            scene.variables.set(node, crate::value::Value::F64(value));
        }
        for at in [430, 1_230] {
            scene.after(ms(at), |scene| async move {
                scene.variables.change(Quality::GOOD);
            });
        }
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = &outcome.frames;
    assert_eq!(frames.len(), 21);
    for frame in frames {
        let keys: Vec<_> = frame.keys().copied().collect();
        assert_eq!(keys, [1, 2, 3, 4]);
    }
    for (key, node) in [(2, A), (3, B), (4, C)] {
        assert_eq!(values(frames, key), served(&outcome, node), "{node}");
    }
    assert_eq!(values(&frames[..1], 2), [10.0]);
    assert_eq!(values(&frames[..1], 3), [20.0]);
    assert_eq!(values(&frames[20..], 4), [32.0]);
    let stamps: Vec<_> = frames.iter().map(|frame| stamp(frame, 1)).collect();
    for pair in stamps.windows(2) {
        assert_eq!(pair[1] - pair[0], ms(100).nanos(), "{stamps:?}");
    }
    let samples = u64::try_from(frames.len() * 3).expect("a few");
    assert_eq!(last(&outcome.statuses, IN), samples);
    assert_eq!(last(&outcome.statuses, MISSED), 0);
    assert_eq!(last(&outcome.statuses, LOST), 0);
}

/// The diagnostics of a config error.
fn diagnostics(result: Result<(), Error>) -> Vec<Diagnostic> {
    match result {
        Err(Error::Config(diagnostics)) => diagnostics,
        other => panic!("a config error: {other:?}"),
    }
}

/// Opens a writer of `channels` at `authority`.
async fn open_rival(hub: &hub::Hub, authority: Authority, channels: &[&str]) -> Writer {
    let config = hub::writer::Config {
        subject: name("plant.rival"),
        authority,
        lease: None,
        channels: channels.iter().map(|&channel| name(channel)).collect(),
    };
    hub.writer(config).await.expect("opens")
}

/// Opens a writer of `channels` at `Authority::ABSOLUTE`, and writes through it one
/// frame with 0 in each value, in which each index of `leads` is stamped its lead
/// after the hub's time. Panics unless the home applies each group.
async fn write_ahead(
    hub: &hub::Hub,
    channels: &[&str],
    leads: &[(u128, Span)],
) -> Writer {
    let mut rival = open_rival(hub, Authority::ABSOLUTE, channels).await;
    let set = Arc::clone(rival.set());
    let entries = set.entries();
    let series: Vec<_> = (0..entries.len()).map(|entry| (entry, 8)).collect();
    let mut draft = rival.draft(Form::Raw, &series).expect("a frame");
    let now = rival.now().nanos();
    for (entry, bytes) in draft.iter_mut() {
        let key = entries[entry].key.as_u128();
        let sample = match leads.iter().find(|(index, _)| *index == key) {
            Some((_, lead)) => (now + lead.nanos()).to_le_bytes(),
            None => 0.0_f64.to_le_bytes(),
        };
        bytes.copy_from_slice(&sample);
    }
    for &index in set.groups() {
        draft.set_count(entries[index].group, 1);
    }
    let outcomes = rival.write(Label::Path(Path::Live), draft);
    let applied = outcomes.as_ref().is_ok_and(|outcomes| {
        outcomes.len() == set.groups().len()
            && outcomes
                .iter()
                .all(|outcome| matches!(outcome, Outcome::Applied { .. }))
    });
    assert!(applied, "each group ahead applies: {outcomes:?}");
    rival
}

/// Drafts frames of `writer` until the pool has no block, and gives them.
fn fill(writer: &Writer) -> Vec<Draft> {
    let mut held = Vec::new();
    for len in [1 << 16, 1 << 12, 1 << 8, 8] {
        let series: Vec<_> = (0..writer.set().entries().len())
            .map(|entry| (entry, len))
            .collect();
        let error = loop {
            match writer.draft(Form::Raw, &series) {
                Ok(draft) => held.push(draft),
                Err(error) => break error,
            }
        };
        assert!(
            matches!(error, frame::Error::Pool(block::Error::Exhausted { .. })),
            "{error}"
        );
    }
    held
}

/// The server gives the wall time of the node, so the client finds the time of each
/// channel token near its own only when its loop reads the mesh time of the node.
#[test]
fn the_loop_of_the_kind_reads_the_mesh_time_of_its_node() {
    let name = "kind::tests::writes_one_stamp_in_each_index_of_a_frame";
    let output = crate::child::output(name, &[]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("test result: ok. 1 passed"), "{stdout}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains("CreatedAt"), "{stderr}");
}

#[test]
fn writes_one_stamp_in_each_index_of_a_frame() {
    let mut setup = Setup::three();
    setup.channels.extend([("plant.other", index(10))]);
    setup.channels[3] = ("plant.c", data(4, 10, F64));
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let (frames, other) = (&outcome.frames, &outcome.indexes[&10]);
    assert_eq!((frames.len(), other.len()), (11, 11));
    for (frame, other) in frames.iter().zip(other) {
        let keys: Vec<_> = frame.keys().chain(other.keys()).copied().collect();
        assert_eq!(keys, [1, 2, 3, 4, 10]);
        assert_eq!(stamp(frame, 1), stamp(other, 10));
    }
    let samples = u64::try_from(frames.len() * 3).expect("a few");
    assert_eq!(last(&outcome.statuses, IN), samples);
}

#[test]
fn counts_the_ticks_that_a_pause_of_the_node_skipped() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.node.pause(Span::SECOND);
        });
        scene.supervise(ms(3_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let missed = last(&outcome.statuses, MISSED);
    assert_eq!(missed, 9);
    let frames = u64::try_from(outcome.frames.len()).expect("a few");
    assert_eq!(frames + missed, 31);
    assert_eq!(last(&outcome.statuses, IN), frames * 3);
    assert_eq!(last(&outcome.statuses, LOST), 0);
}

#[test]
fn ends_the_run_with_a_device_error_when_the_server_stops_and_reads_again_after() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move { scene.stop().await });
        scene.after(ms(3_050), |scene| async move { scene.serve() });
        scene.supervise(ms(8_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let errors = errors(&outcome.statuses);
    let connect =
        "the connect to opc.tcp://10.0.0.1:4840 failed with BadConnectionRejected";
    assert_eq!(
        errors,
        [
            (
                DEVICE,
                "the read failed with BadSecureChannelClosed".to_owned()
            ),
            (DEVICE, connect.to_owned()),
        ]
    );
    let start = stamp(&outcome.frames[0], 1);
    let stamps = outcome.frames.iter().map(|frame| stamp(frame, 1) - start);
    let (before, after): (Vec<_>, Vec<_>) =
        stamps.partition(|&at| at <= ms(1_000).nanos());
    assert_eq!((before.len(), after.len()), (11, 46));
    assert!(after[0] > ms(3_050).nanos(), "{after:?}");
    assert_eq!(last(&outcome.statuses, RESTARTS), 2);
}

/// The counts hold across the runs of one supervisor call, as `status::Count` does.
#[test]
fn counts_the_samples_of_each_run_after_a_restart() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move { scene.stop().await });
        scene.after(ms(3_050), |scene| async move { scene.serve() });
        scene.supervise(ms(5_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(last(&outcome.statuses, RESTARTS), 2);
    let frames = u64::try_from(outcome.frames.len()).expect("a few");
    assert_eq!(last(&outcome.statuses, IN), frames * 3);
}

/// `missed` and `lost` add the ticks and samples of the last run to those of the runs
/// before it.
#[test]
fn adds_the_missed_ticks_and_lost_samples_of_each_run_to_those_before() {
    let outcome = run(Setup::three(), |scene| async move {
        let rival = write_ahead(&scene.hub, DATA, &[(1, Span::ZERO)]).await;
        scene.after(ms(1_050), move |scene| async move {
            drop(rival);
            scene.node.pause(Span::SECOND);
        });
        scene.after(ms(2_550), |scene| async move { scene.stop().await });
        scene.after(ms(4_050), |scene| async move { scene.serve() });
        scene.after(ms(5_550), |scene| async move {
            let rival = open_rival(&scene.hub, Authority(1), DATA).await;
            let held = fill(&rival);
            scene.node.clock().sleep(Span::SECOND).await;
            drop(held);
            drop(rival);
            scene.node.pause(Span::SECOND);
        });
        scene.supervise(ms(9_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(last(&outcome.statuses, RESTARTS), 2);
    assert_eq!(last(&outcome.statuses, MISSED), 18);
    assert_eq!(last(&outcome.statuses, LOST), 63);
}

#[test]
fn leaves_out_and_counts_each_value_that_is_not_good() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.variables.set_quality(A, Quality(0x8000_0000));
            scene.variables.set_quality(B, Quality(0x4000_0000));
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(errors(&outcome.statuses), []);
    let frames = &outcome.frames;
    let mut expected = vec![vec![1, 2, 3, 4]; 11];
    expected.extend(vec![vec![1, 4]; 10]);
    assert_eq!(keys(frames), expected);
    assert_eq!(values(frames, 4), served(&outcome, C));
    let statuses = &outcome.statuses;
    assert_eq!(last(statuses, BAD), 10);
    assert_eq!(last(statuses, UNCERTAIN), 10);
    assert_eq!(last(statuses, MISTYPED), 0);
    assert_eq!(last(statuses, IN), 11 * 3 + 10);
}

#[test]
fn writes_the_value_of_a_good_status_with_a_subcode() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.variables.change(Quality(0x0096_0000));
        });
        scene.supervise(ms(1_550)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(errors(&outcome.statuses), []);
    let frames = &outcome.frames;
    assert_eq!(frames.len(), 16);
    assert_eq!(values(frames, 2), served(&outcome, A));
}

/// The error names the read of the first bad status, also past the first read.
#[test]
fn counts_a_node_that_the_server_does_not_have_as_bad() {
    let mut setup = Setup::three();
    setup.reads[1].1 = "ns=3;s=Missing".into();
    let outcome = run(setup, |scene| async move { scene.supervise(ms(350)).await });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(errors(&outcome.statuses), []);
    assert_eq!(keys(&outcome.frames), vec![vec![1, 2, 4]; 4]);
    assert_eq!(last(&outcome.statuses, BAD), 4);
}

/// A tick that leaves out each value writes no frame, and the frames start again
/// with the first Good value.
#[test]
fn writes_no_frame_when_each_value_is_left_out() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.variables.change(Quality(0x8000_0000));
        });
        scene.after(ms(1_550), |scene| async move {
            scene.variables.change(Quality::GOOD);
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = &outcome.frames;
    assert_eq!(keys(frames), vec![vec![1, 2, 3, 4]; 16]);
    let start = stamp(&frames[0], 1);
    assert_eq!(stamp(&frames[11], 1) - start, ms(1_600).nanos());
    assert_eq!(last(&outcome.statuses, BAD), 15);
    assert_eq!(last(&outcome.statuses, IN), 16 * 3);
}

/// A group whose data series a tick leaves out keeps its index and its other
/// series, and a group with no series left writes no stamp.
#[test]
fn writes_the_index_of_each_group_with_a_value_left() {
    let mut setup = Setup::three();
    setup.channels.extend([("plant.other", index(10))]);
    setup.channels[3] = ("plant.c", data(4, 10, F64));
    let outcome = run(setup, |scene| async move {
        scene.variables.set_quality(A, Quality(0x8000_0000));
        scene.after(ms(550), |scene| async move {
            scene.variables.set_quality(C, Quality(0x8000_0000));
        });
        scene.supervise(ms(1_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let (frames, other) = (&outcome.frames, &outcome.indexes[&10]);
    assert_eq!(keys(frames), vec![vec![1, 3]; 11]);
    assert_eq!(keys(other), vec![vec![4, 10]; 6]);
    for (frame, other) in frames.iter().zip(other) {
        assert_eq!(stamp(frame, 1), stamp(other, 10));
    }
    assert_eq!(last(&outcome.statuses, BAD), 11 + 5);
    assert_eq!(last(&outcome.statuses, IN), 11 + 6);
}

#[test]
fn writes_a_frame_refused_as_backwards_again_after_the_stamp_of_the_refusal() {
    let lead = ms(500);
    let outcome = run(Setup::three(), move |scene| async move {
        drop(write_ahead(&scene.hub, DATA, &[(1, lead)]).await);
        scene.supervise(ms(1_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = &outcome.frames;
    let stamps: Vec<_> = frames.iter().map(|frame| stamp(frame, 1)).collect();
    let [ahead, first, ..] = stamps[..] else {
        panic!("two frames or more: {stamps:?}");
    };
    assert_eq!(first, ahead + 1);
    assert!(stamps.is_sorted_by(|a, b| a < b), "{stamps:?}");
    let samples = u64::try_from((frames.len() - 1) * 3).expect("a few");
    assert_eq!(last(&outcome.statuses, IN), samples);
    assert_eq!(last(&outcome.statuses, LOST), 0);
}

#[test]
fn refuses_each_channel_whose_samples_are_not_a_scalar_of_a_value() {
    let mut setup = Setup::three();
    setup.channels[1] = ("plant.a", data(2, 1, Type::Scalar(Scalar::Stamp)));
    setup.channels[3] = ("plant.c", data(4, 1, Type::String));
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    let refused = |channel: &str, data_type: &str| {
        Diagnostic::new(
            SAMPLE_TYPE,
            None,
            format!(
                "the channel `{channel}` has samples of type {data_type}, and the \
                 opcua kind reads only bool, i8, u8, i16, u16, i32, u32, i64, u64, \
                 f32, and f64"
            ),
            "Give the channel one of these sample types, or remove its `read` block"
                .into(),
        )
    };
    assert_eq!(
        diagnostics(outcome.result),
        [
            refused("plant.a", "timestamp"),
            refused("plant.c", "string")
        ]
    );
    assert_eq!(outcome.frames, []);
}

/// The nodes of [`TYPED`], one for each type of a value.
const TYPED_NODES: &[&str] = &[
    "ns=2;i=1",
    "ns=2;i=2",
    "ns=2;i=3",
    "ns=2;i=4",
    "ns=2;i=5",
    "ns=2;i=6",
    "ns=2;i=7",
    "ns=2;i=8",
    "ns=2;i=9",
    "ns=2;i=10",
    "ns=2;i=11",
];

/// The channels of [`TYPED`], in the order of [`SCALARS`].
const TYPED_CHANNELS: [&str; 11] = [
    "plant.bool",
    "plant.i8",
    "plant.u8",
    "plant.i16",
    "plant.u16",
    "plant.i32",
    "plant.u32",
    "plant.i64",
    "plant.u64",
    "plant.f32",
    "plant.f64",
];

/// One channel of each type of a value (keys 2 to 12) on `plant.time` (key 1), each
/// reading its node of [`TYPED_NODES`].
fn typed() -> Setup {
    let mut channels = vec![("plant.time", index(1))];
    let mut reads = Vec::new();
    for (at, (&channel, scalar)) in TYPED_CHANNELS.iter().zip(SCALARS).enumerate() {
        let key = u128::try_from(at).expect("small") + 2;
        channels.push((channel, data(key, 1, Type::Scalar(scalar))));
        reads.push((channel, TYPED_NODES[at].to_owned()));
    }
    Setup {
        nodes: TYPED_NODES.iter().map(|&node| node.to_owned()).collect(),
        channels,
        reads,
    }
}

/// The values of each type: the least, the largest, then one more: for a float, the
/// one NaN that open62541 sends, the quiet NaN with the sign bit set.
fn extremes() -> [[crate::value::Value; 11]; 3] {
    use crate::value::Value as V;
    [
        [
            V::Bool(false),
            V::I8(i8::MIN),
            V::U8(u8::MIN),
            V::I16(i16::MIN),
            V::U16(u16::MIN),
            V::I32(i32::MIN),
            V::U32(u32::MIN),
            V::I64(i64::MIN),
            V::U64(u64::MIN),
            V::F32(f32::NEG_INFINITY),
            V::F64(f64::NEG_INFINITY),
        ],
        [
            V::Bool(true),
            V::I8(i8::MAX),
            V::U8(u8::MAX),
            V::I16(i16::MAX),
            V::U16(u16::MAX),
            V::I32(i32::MAX),
            V::U32(u32::MAX),
            V::I64(i64::MAX),
            V::U64(u64::MAX),
            V::F32(f32::INFINITY),
            V::F64(f64::INFINITY),
        ],
        [
            V::Bool(true),
            V::I8(-1),
            V::U8(1),
            V::I16(-1),
            V::U16(1),
            V::I32(-1),
            V::U32(1),
            V::I64(-1),
            V::U64(1),
            V::F32(f32::from_bits(0xffc0_0000)),
            V::F64(f64::from_bits(0xfff8_0000_0000_0000)),
        ],
    ]
}

/// The samples of [`extremes`], each widened to 8 bytes as [`decode`] does.
const EXTREMES: [[u64; 11]; 3] = [
    [
        0,
        0x80,
        0,
        0x8000,
        0,
        0x8000_0000,
        0,
        0x8000_0000_0000_0000,
        0,
        0xff80_0000,
        0xfff0_0000_0000_0000,
    ],
    [
        1,
        0x7f,
        0xff,
        0x7fff,
        0xffff,
        0x7fff_ffff,
        0xffff_ffff,
        0x7fff_ffff_ffff_ffff,
        u64::MAX,
        0x7f80_0000,
        0x7ff0_0000_0000_0000,
    ],
    [
        1,
        0xff,
        1,
        0xffff,
        1,
        0xffff_ffff,
        1,
        u64::MAX,
        1,
        0xffc0_0000,
        0xfff8_0000_0000_0000,
    ],
];

#[test]
fn writes_each_type_of_a_value_into_a_channel_of_its_type() {
    let outcome = run(typed(), |scene| async move {
        let set = |scene: &Scene, values: [crate::value::Value; 11]| {
            for (node, value) in TYPED_NODES.iter().zip(values) {
                scene.variables.set(node, value);
            }
        };
        let [least, largest, more] = extremes();
        set(&scene, least);
        scene.after(ms(430), move |scene| async move { set(&scene, largest) });
        scene.after(ms(830), move |scene| async move { set(&scene, more) });
        scene.supervise(ms(1_250)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(errors(&outcome.statuses), []);
    let mut written: Vec<Vec<u64>> = (outcome.frames.iter())
        .map(|frame| {
            (2..=12)
                .map(|key| fixed(frame, key).expect("a series")[0])
                .collect()
        })
        .collect();
    written.dedup();
    assert_eq!(written, EXTREMES);
}

/// The note names the first node of another type of a run, and keeps it when
/// another node is of another type later.
#[test]
fn leaves_out_a_value_of_another_type_and_notes_its_node() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.variables.set(B, crate::value::Value::I32(7));
        });
        scene.after(ms(1_550), |scene| async move {
            scene.variables.set(C, crate::value::Value::F32(1.0));
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    assert_eq!(errors(&outcome.statuses), []);
    let mut expected = vec![vec![1, 2, 3, 4]; 11];
    expected.extend(vec![vec![1, 2, 4]; 5]);
    expected.extend(vec![vec![1, 2]; 5]);
    assert_eq!(keys(&outcome.frames), expected);
    let note = "leaves out the values of the node `ns=3;i=7` of the channel `plant.b`: \
                they are of type Int32, and the channel has samples of type f64";
    assert_eq!(notes(&outcome.statuses), [note]);
    assert_eq!(last(&outcome.statuses, MISTYPED), 10 + 5);
    assert_eq!(last(&outcome.statuses, BAD), 0);
}

#[test]
fn leaves_out_a_value_of_another_type_of_the_same_width() {
    let outcome = run(typed(), |scene| async move {
        let [_, _, values] = extremes();
        for (node, value) in TYPED_NODES.iter().zip(values) {
            scene.variables.set(node, value);
        }
        scene.after(ms(1_050), |scene| async move {
            scene
                .variables
                .set(TYPED_NODES[9], crate::value::Value::I32(1));
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let held = |frame: &Frame| frame.contains_key(&11);
    let held: Vec<_> = outcome.frames.iter().map(held).collect();
    let mut expected = vec![true; 11];
    expected.extend([false; 10]);
    assert_eq!(held, expected);
    let note = "leaves out the values of the node `ns=2;i=10` of the channel \
                `plant.f32`: they are of type Int32, and the channel has samples of \
                type f32";
    assert_eq!(notes(&outcome.statuses), [note]);
    assert_eq!(last(&outcome.statuses, MISTYPED), 10);
}

/// A node whose value is no scalar of a sample type adds 1 to `mistyped` each tick,
/// and the note names its OPC UA type.
#[test]
fn leaves_out_a_node_whose_value_is_no_scalar_of_a_sample_type() {
    use crate::sim::Fixed;
    for (fixed, type_) in [
        (Fixed::String, "String"),
        (Fixed::Doubles, "array of Double"),
    ] {
        let outcome = run(Setup::three(), move |scene| async move {
            scene.stop().await;
            scene.variables.fix(B, fixed);
            scene.serve();
            scene.supervise(ms(1_050)).await
        });
        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        assert_eq!(keys(&outcome.frames), vec![vec![1, 2, 4]; 11], "{type_}");
        let note = format!(
            "leaves out the values of the node `ns=3;i=7` of the channel `plant.b`: \
             they are of type {type_}, and the channel has samples of type f64"
        );
        assert_eq!(notes(&outcome.statuses), [note]);
        assert_eq!(last(&outcome.statuses, MISTYPED), 11, "{type_}");
    }
}

/// Three nodes whose names together are past one message of the server (524047
/// bytes) go in two requests, and each tick reads each node.
#[test]
fn reads_each_node_on_each_tick_when_the_nodes_need_two_requests() {
    let long = |letter: &str| format!("ns=3;s={letter}{}", "x".repeat(200_000));
    let nodes = [long("a"), long("b"), long("c")];
    let mut setup = Setup::three();
    setup.nodes = nodes.to_vec();
    for (read, node) in setup.reads.iter_mut().zip(&nodes) {
        read.1.clone_from(node);
    }
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = &outcome.frames;
    assert_eq!(frames.len(), 11);
    for (key, node) in [2, 3, 4].into_iter().zip(&nodes) {
        assert_eq!(values(frames, key), served(&outcome, node), "{key}");
    }
    assert_eq!(last(&outcome.statuses, IN), 33);
}

#[test]
fn refuses_a_node_whose_read_alone_is_past_one_message_of_the_server() {
    let mut setup = Setup::three();
    setup.reads[1].1 = format!("ns=3;s={}", "x".repeat(600_000));
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    let node = format!("ns=3;s={}", "x".repeat(600_000));
    assert_eq!(
        diagnostics(outcome.result),
        [Diagnostic::new(
            LARGE_READ,
            None,
            format!(
                "a Read of the node `{node}` of the channel `plant.b` needs 600087 bytes, \
                 past the 524047 bytes of one message of the server"
            ),
            "Raise the message size of the server".into(),
        )]
    );
    assert_eq!(outcome.frames, []);
}

#[test]
fn counts_the_samples_of_a_frame_refused_while_another_writer_holds_control() {
    let outcome = run(Setup::three(), |scene| async move {
        let rival = write_ahead(&scene.hub, DATA, &[(1, Span::ZERO)]).await;
        scene.after(ms(1_050), move |_| async move { drop(rival) });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = u64::try_from(outcome.frames.len()).expect("a few") - 1;
    assert_eq!(last(&outcome.statuses, IN), frames * 3);
    assert_eq!(last(&outcome.statuses, LOST), 33);
}

#[test]
fn counts_the_samples_of_a_frame_for_which_the_pool_has_no_room() {
    let outcome = run(Setup::three(), |scene| async move {
        let rival = open_rival(&scene.hub, Authority(1), DATA).await;
        scene.after(ms(550), move |scene| async move {
            let held = fill(&rival);
            scene.node.clock().sleep(Span::SECOND).await;
            drop(held);
            drop(rival);
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = u64::try_from(outcome.frames.len()).expect("a few");
    assert_eq!(frames, 11);
    assert_eq!(last(&outcome.statuses, IN), frames * 3);
    assert_eq!(last(&outcome.statuses, LOST), 30);
}

#[test]
fn closes_the_session_at_the_end_of_each_run() {
    let outcome = run(Setup::three(), |scene| async move {
        let mut reads = Vec::new();
        for _ in 0..5 {
            scene.supervise(ms(550)).await.expect("ok");
            reads.push(scene.variables.served()[A].len());
        }
        assert_eq!(reads, [6, 12, 18, 24, 30]);
        Ok(())
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
}

#[test]
fn closes_the_session_at_each_end_with_an_error() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.variables.change(Quality(0x8000_0000));
        scene.supervise(ms(30_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let bad = "the node `ns=3;s=SpikeData` of the channel `plant.a` gives Bad";
    assert_eq!(errors(&outcome.statuses), [(DEVICE, bad.to_owned())]);
    assert_eq!(last(&outcome.statuses, RESTARTS), 5);
}

/// Each refused group is written again after the latest stamp that the home gives,
/// whichever index has it.
#[test]
fn writes_groups_refused_as_backwards_again_after_the_latest_stamp() {
    for (one, ten) in [(300, 600), (600, 300)] {
        let mut setup = Setup::three();
        setup.channels.push(("plant.other", index(10)));
        setup.channels[3] = ("plant.c", data(4, 10, F64));
        let outcome = run(setup, move |scene| async move {
            let leads = [(1, ms(one)), (10, ms(ten))];
            drop(write_ahead(&scene.hub, DATA, &leads).await);
            scene.supervise(ms(1_050)).await
        });
        assert!(outcome.result.is_ok(), "{:?}", outcome.result);
        let (frames, other) = (&outcome.frames, &outcome.indexes[&10]);
        let ahead = stamp(&frames[0], 1).max(stamp(&other[0], 10));
        assert_eq!(stamp(&frames[1], 1), ahead + 1, "{one} {ten}");
        assert_eq!(stamp(&other[1], 10), ahead + 1, "{one} {ten}");
    }
}

/// When the home refuses only the second index as `Backwards`, the run writes only
/// that group again.
#[test]
fn writes_only_the_refused_second_group_again() {
    let mut setup = Setup::three();
    setup.channels.push(("plant.other", index(10)));
    setup.channels[3] = ("plant.c", data(4, 10, F64));
    let outcome = run(setup, move |scene| async move {
        drop(write_ahead(&scene.hub, &["plant.c"], &[(10, ms(600))]).await);
        scene.supervise(ms(1_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let (frames, other) = (&outcome.frames, &outcome.indexes[&10]);
    let ahead = stamp(&other[0], 10);
    assert_eq!(stamp(&other[1], 10), ahead + 1);
    let stamps: Vec<_> = other.iter().map(|frame| stamp(frame, 10)).collect();
    assert!(stamps.is_sorted_by(|a, b| a < b), "{stamps:?}");
    assert_eq!(frames.len(), other.len() - 1);
    let samples = u64::try_from(frames.len() * 3).expect("a few");
    assert_eq!(last(&outcome.statuses, IN), samples);
    assert_eq!(last(&outcome.statuses, LOST), 0);
}

#[test]
fn counts_the_samples_of_a_frame_that_the_home_loses() {
    let outcome = run(Setup::three(), |scene| async move {
        let rival = open_rival(&scene.hub, Authority(1), DATA).await;
        scene.after(ms(550), move |scene| async move {
            let mut held = fill(&rival);
            drop(held.pop());
            scene.node.clock().sleep(Span::SECOND).await;
            drop(held);
            drop(rival);
        });
        scene.supervise(ms(2_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let frames = u64::try_from(outcome.frames.len()).expect("a few");
    assert_eq!(frames, 11);
    assert_eq!(last(&outcome.statuses, IN), frames * 3);
    assert_eq!(last(&outcome.statuses, LOST), 30);
}

/// The read of one node has no delay, so the request starts and ends at the time of
/// the answer of the server.
#[test]
fn stamps_each_frame_within_its_read() {
    let offset = Arc::new(AtomicI64::new(0));
    let mesh = Arc::clone(&offset);
    let outcome = run(Setup::three(), |scene| async move {
        let rival = open_rival(&scene.hub, Authority(1), DATA).await;
        let monotonic = i64::try_from(scene.node.clock().now().0).expect("small");
        mesh.store(rival.now().nanos() - monotonic, Ordering::Relaxed);
        drop(rival);
        scene.supervise(ms(1_050)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let offset = offset.load(Ordering::Relaxed);
    let served = &outcome.served[A];
    assert_eq!(outcome.frames.len(), served.len());
    for (frame, served) in outcome.frames.iter().zip(served) {
        let answered = i64::try_from(served.time.0).expect("small") + offset;
        assert_eq!(stamp(frame, 1), answered);
    }
}

/// Builds `Values` with a value that `Left::sort` leaves out, which no call of the
/// crate can do. This test is the only kill of a `get` that gives a value for a left
/// out read in place of the panic.
#[test]
#[should_panic(
    expected = "invariant: read 2 is Err(Status(BadNotFound)), which `Left::sort` \
                leaves out"
)]
fn panics_with_the_read_whose_value_is_left_out() {
    use crate::value::{Fault, Value};
    let bad = Err(Fault::Status(Status::BAD_NOT_FOUND));
    let values = [Ok(Value::F64(1.0)), bad, bad];
    Values(&values).get(2);
}

#[test]
fn leaves_out_each_value_of_another_type_than_its_channel() {
    let [_, _, values] = extremes();
    for value in values {
        for sample in SCALARS {
            let variant = crate::value::Value::variant(sample).expect("a type");
            let left = Leave::of(&Ok(value), variant);
            if value.scalar() == sample {
                assert_eq!(left, None);
            } else {
                let mistyped = Leave::Mistyped(value.built_in());
                assert_eq!(left, Some(mistyped), "{value:?} into {sample:?}");
            }
        }
    }
}

#[test]
fn leaves_out_each_value_that_is_not_good() {
    use crate::value::{Fault, Value};
    let variant = Value::variant(Scalar::F64).expect("a type");
    let leave = |status: u32| Leave::of(&Err(Fault::Status(Status(status))), variant);
    assert_eq!(leave(0x4000_0000), Some(Leave::Uncertain));
    assert_eq!(leave(0x7FFF_FFFF), Some(Leave::Uncertain));
    assert_eq!(leave(Status::BAD_NOT_FOUND.0), Some(Leave::Bad));
    assert_eq!(leave(0xC000_0000), Some(Leave::Bad));
    assert_eq!(leave(0x8000_0000), Some(Leave::Bad));
    let string = crate::value::BuiltIn::new(11);
    let left = Leave::of(&Err(Fault::Type(string)), variant);
    assert_eq!(left, Some(Leave::Mistyped(string)));
}
