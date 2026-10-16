#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::poll_fn;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ::sim::{Sim, node};
use connector::cancel::Token;
use connector::kind::Kind as _;
use connector::status::{self, Class, Reading};
use connector::testing::Rig;
use document::value::{Kind as Of, Value};
use document::{Attribute, Block, Label, Map};
use env::net::Listener;
use env::tasks::Tasks;
use hub::reader::{Mode, Received};
use spec::channel::{Channel, Data};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::channel;
use types::name::{Name, Selector};
use types::sample::Scalar;

use super::*;
use crate::device::Device;
use crate::poll::Order;
use crate::tcp::serve;

/// An attribute of `key` with `kind` and no span.
fn attribute(key: &str, kind: Of) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value: Value { kind, span: None },
    }
}

/// The config of a connector at `plc` with a `read` block of each channel.
fn config(channels: &[&str]) -> Document {
    let read = |(address, channel): (i128, &&str)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![Label {
            text: (*channel).into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![
                attribute("table", Of::String("holding".into())),
                attribute("address", Of::Integer(address)),
            ])
            .expect("unique keys"),
            blocks: Vec::new(),
        },
        span: None,
    };
    Document {
        attributes: Map::new(vec![
            attribute("address", Of::String("plc".into())),
            attribute("rate", Of::String("10Hz".into())),
        ])
        .expect("unique keys"),
        blocks: (0..).zip(channels).map(read).collect(),
    }
}

#[test]
fn check_gives_each_channel_as_a_write_in_order_and_the_counts() {
    let config = Kind
        .parse(&config(&["plc.temp", "plc.flow"]))
        .expect("a config");
    let names = |names: &[&str]| -> Vec<_> {
        names
            .iter()
            .map(|name| name.parse().expect("a name"))
            .collect()
    };
    assert_eq!(
        Kind.check(&config),
        Ok(Channels {
            reader: None,
            writes: names(&["plc.temp", "plc.flow"]),
            counts: names(&["in", "missed", "lost"]),
        })
    );
}

#[test]
fn the_table_of_kinds_checks_the_kind() {
    let table = kind::Table::new().with("modbus", Kind);
    let checked = table.check("modbus", None, &config(&["plc.temp"]));
    assert_eq!(checked.map(|channels| channels.writes.len()), Ok(1));
}

#[test]
fn parse_reads_the_config() {
    let refused = Kind.parse(&Document::default()).map(|_| ());
    let codes =
        refused.map_err(|all| all.iter().map(|one| one.code).collect::<Vec<_>>());
    let code = document::diagnostic::Code::new;
    let missing = code("document.missing-attribute");
    assert_eq!(codes, Err(vec![missing, missing, code("modbus.no-read")]));
}

#[test]
fn discover_finds_nothing() {
    let token = cancel::Token::new();
    let found = Kind.discover(&token);
    let found = std::pin::pin!(found);
    let waker = std::task::Waker::noop();
    let polled = found.poll(&mut std::task::Context::from_waker(waker));
    assert!(
        matches!(polled, std::task::Poll::Ready(Ok(ref found)) if found.is_empty()),
        "{polled:?}"
    );
}

/// The connector of each scene.
const PLC: &str = "plc.device";

/// The unit that the device of each scene serves.
const UNIT: u8 = 1;

const fn ms(n: i64) -> Span {
    Span::from_nanos(n * 1_000_000)
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// One read of a scene: its channel, the sample type of the channel, its table as
/// the config names it, its address, and the value that the device holds there,
/// widened to 8 bytes as the frames give it.
#[derive(Clone, Copy)]
struct Item {
    channel: &'static str,
    sample: Type,
    table: &'static str,
    address: u16,
    bits: u64,
}

const fn item(
    channel: &'static str,
    sample: Scalar,
    table: &'static str,
    address: u16,
    bits: u64,
) -> Item {
    Item {
        channel,
        sample: Type::Scalar(sample),
        table,
        address,
        bits,
    }
}

/// A read of each type of each table, with values whose words all differ.
const ITEMS: [Item; 10] = [
    item("plc.coil", Scalar::Bool, "coil", 3, 1),
    item("plc.discrete", Scalar::Bool, "discrete", 5, 1),
    item("plc.u16", Scalar::U16, "holding", 0, 0xbeef),
    item("plc.i16", Scalar::I16, "input", 0, 0xfffe),
    item("plc.u32", Scalar::U32, "holding", 1, 0x1234_5678),
    item("plc.i32", Scalar::I32, "input", 1, 0xffff_fffd),
    item("plc.f32", Scalar::F32, "holding", 3, 0x4049_0fdb),
    item("plc.u64", Scalar::U64, "holding", 5, 0x0102_0304_0506_0708),
    item("plc.i64", Scalar::I64, "input", 3, 0xffff_ffff_ffff_fff9),
    item("plc.f64", Scalar::F64, "holding", 9, 0x4005_bf0a_8b14_5769),
];

/// A device of 100 items in each table that holds each of `items` in `order`, and 0
/// in each other item.
fn device(items: &[Item], order: Order) -> Device {
    let mut device = Device {
        coils: vec![false; 100],
        discrete_inputs: vec![false; 100],
        holding_registers: vec![0; 100],
        input_registers: vec![0; 100],
    };
    for item in items {
        let at = usize::from(item.address);
        let registers = match item.table {
            "coil" => {
                device.coils[at] = item.bits == 1;
                continue;
            }
            "discrete" => {
                device.discrete_inputs[at] = item.bits == 1;
                continue;
            }
            "holding" => &mut device.holding_registers,
            _ => &mut device.input_registers,
        };
        let Type::Scalar(scalar) = item.sample else {
            panic!("a scalar")
        };
        let count = scalar.width() / 2;
        for word in 0..count {
            let shift = match order {
                Order::Big => 16 * (count - 1 - word),
                Order::Little => 16 * word,
            };
            registers[at + word] =
                u16::try_from(item.bits >> shift & 0xffff).expect("a word");
        }
    }
    device
}

/// The config of [`PLC`] at `address` with a `read` block of each of `items`, and
/// `others`.
fn connector(address: &str, items: &[Item], others: &[(&str, Of)]) -> Document {
    let read = |item: &Item| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![Label {
            text: item.channel.into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![
                attribute("table", Of::String(item.table.into())),
                attribute("address", Of::Integer(item.address.into())),
            ])
            .expect("unique keys"),
            blocks: Vec::new(),
        },
        span: None,
    };
    let mut attributes = vec![
        attribute("address", Of::String(address.into())),
        attribute("rate", Of::String("10Hz".into())),
    ];
    attributes.extend(
        others
            .iter()
            .map(|(key, kind)| attribute(key, kind.clone())),
    );
    Document {
        attributes: Map::new(attributes).expect("unique keys"),
        blocks: items.iter().map(read).collect(),
    }
}

/// The definitions of the index `plc.time` of key 1, and of the channel of each of
/// `items` on it, with keys from 2 in order.
fn definitions(items: &[Item]) -> Vec<(Name, Definition)> {
    let index = channel::Key::from_u128(1);
    let time = Definition::Channel(Channel {
        key: index,
        kind: spec::channel::Kind::Index {
            error: None,
            control: None,
        },
    });
    let data = (2..).zip(items).map(|(key, item)| {
        let data = Data::new(index, None, DataType::Sample(item.sample), None);
        let kind = spec::channel::Kind::Data(data.expect("no unit"));
        let key = channel::Key::from_u128(key);
        (
            name(item.channel),
            Definition::Channel(Channel { key, kind }),
        )
    });
    std::iter::once((name("plc.time"), time))
        .chain(data)
        .collect()
}

/// The samples of one frame that a reader received, by key, each widened to 8 bytes.
type Frame = BTreeMap<u128, Vec<u64>>;

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
        let width = entry.data_type.width().expect("a fixed width");
        let widen = |chunk: &[u8]| {
            let mut sample = [0; 8];
            sample[..width].copy_from_slice(chunk);
            u64::from_le_bytes(sample)
        };
        frame.insert(entry.key.as_u128(), raw.chunks(width).map(widen).collect());
    }
    frame
}

/// Reads each frame of `names` into the vector it gives, in a task on `tasks`, and
/// gives each to `decode`.
async fn record<T: 'static>(
    hub: &hub::Hub,
    names: &[Name],
    tasks: &Tasks,
    decode: fn(&Received<'_>) -> T,
) -> Rc<RefCell<Vec<T>>> {
    let config = hub::reader::Config {
        select: Selector::new(names.iter().map(Name::as_str)).expect("a selector"),
        mode: Mode::Complete,
        subject: name("plc.test"),
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

/// What a scene recorded.
struct Record {
    /// What the run returned.
    result: Result<(), Error>,
    /// The frames of the index and the channels of the reads.
    frames: Vec<Frame>,
    /// Each status of the connector.
    statuses: Vec<Reading>,
}

impl Record {
    /// The last value of the count `count`.
    fn last(&self, count: &str) -> u64 {
        let last = self.statuses.last().expect("a status");
        last.counts[&name(count)]
    }

    /// The class and the text of each error of the statuses, once for each run of
    /// equal errors.
    fn errors(&self) -> Vec<(Class, String)> {
        let mut errors: Vec<_> = self
            .statuses
            .iter()
            .filter_map(|status| status.error.clone())
            .collect();
        errors.dedup();
        errors
    }

    /// The diagnostics of the config error that ended the run.
    fn diagnostics(&self) -> Vec<Diagnostic> {
        match &self.result {
            Err(Error::Config(diagnostics)) => diagnostics.clone(),
            other => panic!("a config error: {other:?}"),
        }
    }

    /// The stamp of each frame.
    fn stamps(&self) -> Vec<i64> {
        let stamp = |frame: &Frame| match frame[&1][..] {
            [bits] => i64::from_le_bytes(bits.to_le_bytes()),
            ref other => panic!("one stamp: {other:?}"),
        };
        self.frames.iter().map(stamp).collect()
    }
}

/// A node that serves a device at the host `plc`, and runs the connector [`PLC`] of
/// the modbus kind.
struct Scene {
    node: node::Node,
    tasks: Tasks,
    device: Arc<Mutex<Device>>,
    /// The stop of the server that runs.
    server: RefCell<Option<Token>>,
}

impl Scene {
    /// A listener on port 502.
    fn listen(&self) -> Listener {
        let listen = tcp::Listen {
            local: SocketAddr::new(self.node.addresses()[0], 502),
            backlog: 4,
            options: tcp::Options {
                send_buffer_bytes: 1 << 16,
                recv_buffer_bytes: 1 << 16,
                unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
                delayed: false,
            },
        };
        self.node.net().listen(&listen).expect("the port is free")
    }

    /// Serves the device as [`UNIT`] on port 502.
    fn serve(&self) {
        let listener = self.listen();
        let (stop, device) = (Token::new(), Arc::clone(&self.device));
        let until = stop.clone();
        self.tasks.spawn(async move {
            until.race(serve(listener, UNIT, &device)).await;
        });
        *self.server.borrow_mut() = Some(stop);
    }

    /// Accepts one stream on port 502 and keeps it open with no answer, in place of
    /// the server.
    fn hold(&self) {
        let mut listener = self.listen();
        let stop = Token::new();
        let until = stop.clone();
        self.tasks.spawn(async move {
            until
                .race(async move {
                    let stream = poll_fn(|cx| listener.poll_accept(cx)).await;
                    let _held = stream.expect("a stream");
                    std::future::pending::<()>().await;
                })
                .await;
        });
        *self.server.borrow_mut() = Some(stop);
    }

    /// Stops the server, which closes its listener and its stream.
    fn stop(&self) {
        let stop = self.server.borrow_mut().take();
        stop.expect("a server runs").cancel();
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

/// Runs `script` on the scene, serves `device` when `script` starts no server or
/// hold, and runs [`PLC`] with `config` and channels of `items` for `span`. Records the frames and the statuses
/// until 1 s after the run ends.
fn run<F>(
    device: Device,
    items: &'static [Item],
    config: Document,
    span: Span,
    script: impl FnOnce(Rc<Scene>) -> F + Send + 'static,
) -> Record
where
    F: Future<Output = ()> + 'static,
{
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let answer = ::sim::name::Answer::Addresses(vec![node.addresses()[0]]);
    let lookup = ::sim::name::Config {
        answer,
        ..::sim::name::Config::default()
    };
    sim.name("plc", lookup);
    let run = sim.run_on(&node, move |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let kinds = kind::Table::new().with("modbus", Kind);
        let mut rig = Rig::open(env, node.net(), kinds).await;
        let definitions = definitions(items);
        let names: Vec<_> = definitions.iter().map(|(name, _)| name.clone()).collect();
        rig.define(definitions);
        let frames = record(rig.hub(), &names, &tasks, decode).await;
        let scene = Rc::new(Scene {
            device: Arc::new(Mutex::new(device)),
            server: RefCell::new(None),
            node: node.clone(),
            tasks: tasks.clone(),
        });
        script(Rc::clone(&scene)).await;
        if scene.server.borrow().is_none() {
            scene.serve();
        }
        let connector = rig.supervise("modbus", name(PLC), config).await;
        let counts = COUNTS.map(name);
        let (time, channels) = status::channels(&name(PLC), &counts).expect("names");
        let names: Vec<_> = std::iter::once(time)
            .chain(channels.into_iter().map(|(name, _)| name))
            .collect();
        let status = |received: &Received<'_>| Reading::of(received).expect("a status");
        let statuses = record(rig.hub(), &names, &tasks, status).await;
        node.clock().sleep(span).await;
        let result = connector.stop().await;
        node.clock().sleep(Span::SECOND).await;
        (result, frames.take(), statuses.take())
    });
    let (result, frames, statuses) = run.expect("the run ends");
    Record {
        result,
        frames,
        statuses,
    }
}

/// The keys of the index and of each of `items`, in order.
fn keys(items: &[Item]) -> Vec<u128> {
    (1..).take(items.len() + 1).collect()
}

/// The value of each read in the order of `items`, keyed as [`definitions`] keys
/// them.
fn values(items: &[Item]) -> Vec<(u128, Vec<u64>)> {
    (2..)
        .zip(items)
        .map(|(key, item)| (key, vec![item.bits]))
        .collect()
}

fn assert_frames(record: &Record, items: &[Item], frames: usize) {
    assert_eq!(record.frames.len(), frames);
    for frame in &record.frames {
        assert_eq!(frame.keys().copied().collect::<Vec<_>>(), keys(items));
        let data: Vec<_> = frame
            .iter()
            .filter(|(key, _)| **key != 1)
            .map(|(key, samples)| (*key, samples.clone()))
            .collect();
        assert_eq!(data, values(items));
    }
}

#[test]
fn writes_the_value_of_each_read_of_each_table_and_type_in_one_frame_per_tick() {
    let record = run(
        device(&ITEMS, Order::Big),
        &ITEMS,
        connector("plc:502", &ITEMS, &[]),
        ms(1_050),
        |_| async {},
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    assert_frames(&record, &ITEMS, 11);
    let stamps = record.stamps();
    for pair in stamps.windows(2) {
        assert_eq!(pair[1] - pair[0], ms(100).nanos(), "{stamps:?}");
    }
    assert_eq!(record.last("in"), 11 * 10);
    assert_eq!((record.last("missed"), record.last("lost")), (0, 0));
    assert_eq!(record.errors(), []);
}

#[test]
fn reads_a_value_of_more_than_one_register_in_the_little_order() {
    let record = run(
        device(&ITEMS, Order::Little),
        &ITEMS,
        connector("plc", &ITEMS, &[("order", Of::String("little".into()))]),
        ms(250),
        |_| async {},
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    assert_frames(&record, &ITEMS, 3);
}

#[test]
fn reads_each_value_again_at_each_tick() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 0, 7)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("plc", &ONE, &[]),
        ms(250),
        |scene| async move {
            scene.after(ms(150), |scene| async move {
                scene.device.lock().expect("no panic").holding_registers[0] = 8;
            });
        },
    );
    let values: Vec<_> = record.frames.iter().map(|frame| frame[&2][0]).collect();
    assert_eq!(values, [7, 7, 8]);
}

#[test]
fn ends_with_a_config_error_for_each_read_of_a_type_that_its_table_does_not_hold() {
    const WRONG: [Item; 4] = [
        item("plc.flag", Scalar::F64, "coil", 3, 0),
        item("plc.u16", Scalar::U16, "holding", 0, 0),
        item("plc.far", Scalar::U32, "holding", 65_535, 0),
        Item {
            channel: "plc.pair",
            sample: Type::Array {
                element: Scalar::U16,
                len: 2,
            },
            table: "input",
            address: 0,
            bits: 0,
        },
    ];
    let record = run(
        device(&[], Order::Big),
        &WRONG,
        connector("plc", &WRONG, &[]),
        ms(250),
        |_| async {},
    );
    let refused = |code, message: &str, fix: &str| {
        Diagnostic::new(Code::new(code), None, message.into(), fix.into())
    };
    let types = "Give the channel a sample type that its table holds";
    assert_eq!(
        record.diagnostics(),
        [
            refused(
                "modbus.sample-type",
                "the channel `plc.flag` at address 3 of the coils: the coils hold no \
                 f64 values",
                types,
            ),
            refused(
                "modbus.range",
                "the channel `plc.far` at address 65535 of the holding registers: a \
                 value of 2 registers from address 65535 runs past address 65535",
                "Give the read a lower address, or the channel a type of fewer \
                 registers",
            ),
            refused(
                "modbus.sample-type",
                "the channel `plc.pair` at address 0 of the input registers: the \
                 input registers hold no u16[2] values",
                types,
            ),
        ]
    );
    assert_eq!(record.frames, []);
}

#[test]
fn ends_with_a_config_error_that_names_each_read_of_a_request_that_the_device_refused()
{
    const FAR: [Item; 3] = [
        item("plc.near", Scalar::U16, "holding", 0, 0),
        item("plc.far", Scalar::U32, "holding", 99, 0),
        item("plc.past", Scalar::U16, "holding", 101, 0),
    ];
    let record = run(
        device(&[], Order::Big),
        &FAR,
        connector("plc", &FAR, &[]),
        ms(250),
        |_| async {},
    );
    let refused = |message: &str| {
        let fix = "Read a table and addresses that the device has";
        Diagnostic::new(
            Code::new("modbus.exception"),
            None,
            message.into(),
            fix.into(),
        )
    };
    assert_eq!(
        record.diagnostics(),
        [
            refused(
                "the device refused the read of the channel `plc.far` at address 99 \
                 of the holding registers with exception 2 (illegal data address)"
            ),
            refused(
                "the device refused the read of the channel `plc.past` at address 101 \
                 of the holding registers with exception 2 (illegal data address)"
            ),
        ]
    );
    assert_eq!(record.frames, []);
}

#[test]
fn ends_the_run_with_a_device_error_for_another_exception() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 4, 0)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("plc", &ONE, &[("unit", Of::Integer(2))]),
        ms(250),
        |_| async {},
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    assert_eq!(
        record.errors(),
        [(
            Class::Device,
            "the device refused the read of the holding registers from address 4 with \
             exception 11 (gateway target device failed to respond)"
                .to_owned()
        )]
    );
    assert_eq!(record.frames, []);
}

#[test]
fn ends_the_run_with_a_device_error_when_no_lookup_finds_the_host() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 0, 0)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("nowhere:502", &ONE, &[]),
        ms(250),
        |_| async {},
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    let errors = record.errors();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].0, Class::Device);
    assert!(
        errors[0].1.starts_with("the lookup of nowhere failed: "),
        "{errors:?}"
    );
}

#[test]
fn ends_the_run_with_a_device_error_when_a_reply_is_late() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 0, 0)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("plc", &ONE, &[]),
        ms(1_250),
        |scene| async move { scene.hold() },
    );
    assert_eq!(
        record.errors(),
        [(
            Class::Device,
            "the read of the holding registers from address 0 failed: no whole reply \
             came before the timeout"
                .to_owned()
        )]
    );
    assert_eq!(record.frames, []);
}

#[test]
fn reads_again_in_order_after_the_device_resets_its_stream() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 0, 7)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("plc", &ONE, &[]),
        ms(5_050),
        |scene| async move {
            scene.after(ms(1_050), |scene| async move { scene.stop() });
            scene.after(ms(3_050), |scene| async move { scene.serve() });
        },
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    let errors = record.errors();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert_eq!(
        errors[0],
        (
            Class::Device,
            "the read of the holding registers from address 0 failed: 10.0.0.1:502 \
             reset the stream"
                .to_owned()
        )
    );
    assert_eq!(errors[1].0, Class::Device);
    assert!(
        errors[1]
            .1
            .starts_with("the connect to 10.0.0.1:502 failed: "),
        "{errors:?}"
    );
    let start = record.stamps()[0];
    let stamps: Vec<_> = record.stamps().iter().map(|at| at - start).collect();
    assert!(stamps.is_sorted_by(|a, b| a < b), "{stamps:?}");
    let (before, after): (Vec<i64>, Vec<i64>) =
        stamps.into_iter().partition(|&at| at <= ms(1_000).nanos());
    assert_eq!(before.len(), 11);
    assert!(
        !after.is_empty() && after[0] > ms(3_000).nanos(),
        "{after:?}"
    );
    let frames = u64::try_from(record.frames.len()).expect("a few");
    assert_eq!(record.last("in"), frames);
}

#[test]
fn counts_the_ticks_that_a_pause_of_the_node_skipped() {
    const ONE: [Item; 1] = [item("plc.u16", Scalar::U16, "holding", 0, 7)];
    let record = run(
        device(&ONE, Order::Big),
        &ONE,
        connector("plc", &ONE, &[]),
        ms(3_050),
        |scene| async move {
            scene.after(ms(1_050), |scene| async move {
                scene.node.pause(Span::SECOND);
            });
        },
    );
    assert_eq!(record.result.as_ref().map_err(ToString::to_string), Ok(&()));
    let missed = record.last("missed");
    assert_eq!(missed, 9);
    let frames = u64::try_from(record.frames.len()).expect("a few");
    assert_eq!(frames + missed, 31);
    assert_eq!(record.last("in"), frames);
}
