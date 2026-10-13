#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::rc::Rc;

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
use types::name::Selector;
use types::quality::Quality;
use types::time::Span;

use super::*;
use crate::sim::{Served, Variables, serve};

const A: &str = "ns=3;s=SpikeData";
const B: &str = "ns=3;i=7";
const C: &str = "ns=2;s=Dip";

/// The connector of each scene.
const PLC: &str = "plant.plc";

/// The first key of the status channels of [`PLC`]: the index, then `state`,
/// `class`, `restarts`, `backoff`, `error`, `in`, `missed`, and `lost`.
const STATUS: u128 = 100;
const CLASS: u128 = STATUS + 2;
const RESTARTS: u128 = STATUS + 3;
const ERROR: u128 = STATUS + 5;
const IN: u128 = STATUS + 6;
const MISSED: u128 = STATUS + 7;
const LOST: u128 = STATUS + 8;

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

/// Reads each frame of `names` into the vector it gives, in a task on `tasks`.
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
    let mut reader = hub.reader(config).await.expect("the reader opens");
    let frames = Rc::new(RefCell::new(Vec::new()));
    let into = Rc::clone(&frames);
    tasks.spawn(async move {
        while let Ok(received) = reader.next().await {
            into.borrow_mut().push(decode(&received));
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
    nodes: &'static [&'static str],
    /// The channels other than the status channels, each with its definition.
    channels: Vec<(&'static str, Definition)>,
    /// The channel and the node of each `read` block.
    reads: Vec<(&'static str, String)>,
}

impl Setup {
    /// `plant.a`, `plant.b`, and `plant.c` (keys 2 to 4) on `plant.time` (key 1),
    /// which read [`A`], [`B`], and [`C`].
    fn three() -> Self {
        Self {
            nodes: &[A, B, C],
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
        let (clock, net) = (self.node.clock(), self.node.net());
        let (variables, until) = (self.variables.clone(), stop.clone());
        let mut rng = self.node.entropy().rng();
        group.tasks().spawn(async move {
            serve(clock, net, listener, &mut rng, &variables, until.wait()).await;
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
            variables: Variables::new(setup.nodes),
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
            scene.variables.set(node, value);
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

/// Opens a writer of `plant.a`, `plant.b`, and `plant.c` at `authority`.
async fn open_rival(hub: &hub::Hub, authority: Authority) -> Writer {
    let config = hub::writer::Config {
        subject: name("plant.rival"),
        authority,
        lease: None,
        channels: ["plant.a", "plant.b", "plant.c"].map(name).into(),
    };
    hub.writer(config).await.expect("opens")
}

/// Opens a writer as [`open_rival`] does at `Authority::ABSOLUTE`, and writes through
/// it one frame stamped `lead` after the hub's time, with 0 in each value.
async fn write_ahead(hub: &hub::Hub, lead: Span) -> Writer {
    let mut rival = open_rival(hub, Authority::ABSOLUTE).await;
    let entries = rival.set().entries();
    let series: Vec<_> = (0..entries.len()).map(|entry| (entry, 8)).collect();
    let mut draft = rival.draft(Form::Raw, &series).expect("a frame");
    let stamp = rival.now().nanos() + lead.nanos();
    for (entry, bytes) in draft.iter_mut() {
        let sample = match entries[entry].key.as_u128() {
            1 => stamp.to_le_bytes(),
            _ => 0.0_f64.to_le_bytes(),
        };
        bytes.copy_from_slice(&sample);
    }
    draft.set_count(entries[0].group, 1);
    let outcomes = rival.write(Label::Path(Path::Live), draft);
    let applied = matches!(outcomes, Ok([Outcome::Applied { .. }]));
    assert!(applied, "the frame ahead applies: {outcomes:?}");
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

#[test]
fn ends_the_run_with_a_device_error_on_a_bad_status() {
    let outcome = run(Setup::three(), |scene| async move {
        scene.after(ms(1_050), |scene| async move {
            scene.variables.change(Quality(0x8000_0000));
        });
        scene.supervise(ms(1_550)).await
    });
    assert!(outcome.result.is_ok(), "{:?}", outcome.result);
    let errors = errors(&outcome.statuses);
    let bad = "the node ns=3;s=SpikeData of `plant.a` gives Bad";
    assert_eq!(errors, [(DEVICE, bad.to_owned())]);
    assert_eq!(outcome.frames.len(), 11);
}

#[test]
fn writes_a_frame_refused_as_backwards_again_after_the_stamp_of_the_refusal() {
    let lead = ms(500);
    let outcome = run(Setup::three(), move |scene| async move {
        drop(write_ahead(&scene.hub, lead).await);
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
fn refuses_a_channel_whose_samples_are_not_f64() {
    let mut setup = Setup::three();
    setup.channels[1] = ("plant.a", data(2, 1, Type::Scalar(Scalar::I32)));
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    assert_eq!(
        diagnostics(outcome.result),
        [Diagnostic::new(
            SAMPLE_TYPE,
            None,
            "the channel `plant.a` has samples of type i32, and the opcua kind reads \
             only f64"
                .into(),
            "Give the channel the sample type f64, or remove its `read` block".into(),
        )]
    );
    assert_eq!(outcome.frames, []);
}

#[test]
fn refuses_a_read_past_one_message_of_the_server() {
    let mut setup = Setup::three();
    setup.reads[0].1 = format!("ns=3;s={}", "x".repeat(600_000));
    let outcome = run(
        setup,
        |scene| async move { scene.supervise(ms(1_050)).await },
    );
    assert_eq!(
        diagnostics(outcome.result),
        [Diagnostic::new(
            LARGE_READ,
            None,
            "a Read of 3 nodes needs 600129 bytes, past the 524047 bytes of one \
             message of the server"
                .into(),
            "Read fewer nodes in the connector, or split it into two connectors".into(),
        )]
    );
    assert_eq!(outcome.frames, []);
}

#[test]
fn counts_the_samples_of_a_frame_refused_while_another_writer_holds_control() {
    let outcome = run(Setup::three(), |scene| async move {
        let rival = write_ahead(&scene.hub, Span::ZERO).await;
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
        let rival = open_rival(&scene.hub, Authority(1)).await;
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
