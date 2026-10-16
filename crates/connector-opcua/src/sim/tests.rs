use std::ffi::{CStr, CString};
use std::future;
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};

use ::sim::{Sim, node};
use env::net::{Listener, tcp};
use types::quality::Quality;

use super::{Served, Variables, serve};
use crate::client::Client;
use crate::client::browse::{Class, Description, Page, Reference, Unit};
use crate::event;
use crate::ffi::Status;
use crate::node::{Key, Node};
use crate::value::{Fault, Type, Value};

const SPIKE: &str = "ns=3;s=SpikeData";
const DIP: &str = "ns=3;i=7";
const BAD: Quality = Quality(0x8000_0000);
const CLAMPED: Quality = Quality(0x0030_0000);
const UNCERTAIN: Quality = Quality(0x4090_0000);
const BAD_USER_ACCESS_DENIED: Status = Status(0x801F_0000);
const QUALITIES: [Quality; 4] = [Quality::GOOD, CLAMPED, UNCERTAIN, BAD];
/// The reads of a variable with each of [`QUALITIES`] in turn.
const READS: [Result<Value, Fault>; 4] = [
    Ok(Value::F64(1.0)),
    Ok(Value::F64(2.0)),
    Err(Fault::status(Status(UNCERTAIN.0))),
    Err(Fault::status(Status(BAD.0))),
];

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

#[test]
fn a_server_answers_each_read_with_its_variable_and_records_it() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE, DIP]);
        let (clock, spike) = (node.clock(), SPIKE.parse::<Node>().expect("a node"));
        let mut times = Vec::new();
        let mut reads = Vec::new();
        let until = async {
            let mut client = connect(&node, &mut rng, &url).await.expect("connect");
            for quality in QUALITIES {
                variables.change(quality);
                let before = clock.now();
                reads.push(client.read_one(&spike).await);
                times.push((before, clock.now()));
            }
            client.close().await;
        };
        let mut served = node.entropy().rng();
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
        .await;
        assert_eq!(reads, READS);
        let record = variables.served();
        assert_eq!(record[DIP], []);
        let spike = &record[SPIKE];
        let answers: Vec<_> = (spike.iter())
            .map(|served| (served.value, served.quality))
            .collect();
        let expected: Vec<_> =
            [1.0, 2.0, 3.0, 4.0].into_iter().zip(QUALITIES).collect();
        assert_eq!(answers, expected);
        for (Served { time, .. }, (before, after)) in spike.iter().zip(times) {
            assert!(
                before <= *time && *time <= after,
                "{time:?} in {before:?}..{after:?}"
            );
        }
        let again = listen(&node);
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            again,
            &mut served,
            &variables,
            async {},
        )
        .await;
    })
    .expect("the run ends");
}

#[test]
fn a_server_answers_a_value_of_each_type_and_records_only_a_double() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let values = [
            Value::Bool(true),
            Value::I8(-8),
            Value::U8(8),
            Value::I16(-16),
            Value::U16(16),
            Value::I32(-32),
            Value::U32(32),
            Value::I64(-64),
            Value::U64(64),
            Value::F32(0.5),
            Value::F64(0.25),
        ];
        let texts: Vec<String> =
            (1..=values.len()).map(|i| format!("ns=2;i={i}")).collect();
        let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
        let variables = Variables::new(&texts);
        for (text, value) in texts.iter().zip(values) {
            variables.set(text, value);
        }
        variables.change(Quality::GOOD);
        let mut reads = Vec::new();
        let until = async {
            let mut client = connect(&node, &mut rng, &url).await.expect("connect");
            for text in &texts {
                let node = text.parse::<Node>().expect("a node");
                reads.push(client.read_one(&node).await);
            }
            client.close().await;
        };
        let mut served = node.entropy().rng();
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
        .await;
        let mut expected = values.map(Ok);
        expected[10] = Ok(Value::F64(1.25));
        assert_eq!(reads, expected);
        let record = variables.served();
        for text in &texts[..10] {
            assert_eq!(record[*text], [], "{text}");
        }
        let doubles: Vec<_> = record[texts[10]]
            .iter()
            .map(|served| served.value)
            .collect();
        assert_eq!(doubles, [1.25]);
    })
    .expect("the run ends");
}

#[test]
fn a_server_has_room_for_4_sessions() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE]);
        let spike = SPIKE.parse::<Node>().expect("a node");
        let mut reads = Vec::new();
        let mut fifth = None;
        let until = async {
            let mut clients = Vec::new();
            for _ in 0..4 {
                clients.push(connect(&node, &mut rng, &url).await.expect("connect"));
            }
            for client in &mut clients {
                reads.push(client.read_one(&spike).await);
            }
            fifth = connect(&node, &mut rng, &url).await.err();
            for client in clients {
                client.close().await;
            }
        };
        let mut served = node.entropy().rng();
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
        .await;
        assert_eq!(reads, [Ok(Value::F64(0.0)); 4]);
        assert_eq!(fifth, Some(Status::BAD_CONNECTION_CLOSED));
        assert_eq!(variables.served()[SPIKE].len(), 4);
    })
    .expect("the run ends");
}

#[test]
fn a_server_panics_on_a_variable_of_the_number_0() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE, "ns=3;i=0"]);
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listen(&node),
            &mut rng,
            &variables,
            async {},
        )
        .await;
    });
    let Err(::sim::Error::Panicked { message, .. }) = run else {
        panic!("the serve panics: {run:?}");
    };
    assert_eq!(
        message,
        "ns=3;i=0: open62541 adds a node of the number 0 at a free number"
    );
}

#[test]
#[should_panic(expected = r#""ns=3;x=1" is not a node id: "#)]
fn variables_panic_on_a_text_that_is_not_a_node_id() {
    drop(Variables::new(&[SPIKE, "ns=3;x=1"]));
}

#[test]
#[should_panic(expected = r#""ns=03;i=7" and "ns=3;i=7" name the same node"#)]
fn variables_panic_on_two_texts_of_one_node() {
    drop(Variables::new(&[DIP, "ns=03;i=7"]));
}

/// Serves with a `wall` that panics in a child process, and does nothing outside one.
#[test]
fn serves_with_a_wall_that_panics() {
    if !crate::child::running() {
        return;
    }
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE]);
        let wall = || -> types::time::Stamp { panic!("the wall fails") };
        let (clock, net) = (node.clock(), node.net());
        serve(
            clock,
            wall,
            net,
            listen(&node),
            &mut rng,
            &variables,
            async {},
        )
        .await;
    });
    drop(run);
}

#[test]
fn a_panic_in_the_wall_of_a_server_aborts() {
    let output =
        crate::child::aborted("sim::tests::serves_with_a_wall_that_panics", &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("panic in a function that cannot unwind"),
        "{stderr}"
    );
}

/// Connects a client of `node` to `url`.
/// Serves no variables, and gives the output of `body` with a client of the server.
fn with_tree<T: Send + 'static>(
    body: impl AsyncFnOnce(&mut Client) -> T + Send + 'static,
) -> T {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let mut out = None;
        let until = async {
            let mut client = connect(&node, &mut rng, &url).await.expect("connect");
            out = Some(body(&mut client).await);
            client.close().await;
        };
        let mut served = node.entropy().rng();
        let variables = Variables::new(&[]);
        let wall = event::Time::monotonic(node.clock()).wall;
        serve(
            node.clock(),
            wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
        .await;
        out.expect("the body ends")
    })
    .expect("the run ends")
}

/// The node of the name `name` in namespace `namespace`.
fn named(namespace: u16, name: &str) -> Node {
    format!("ns={namespace};s={name}").parse().expect("a node")
}

fn ns0(number: u32) -> Node {
    Node {
        namespace: 0,
        key: Key::Number(number),
    }
}

/// A reference of the type `via` in namespace 0 to the node `name` of namespace 1,
/// with the browse and display name `text`.
fn reference(name: &str, text: &str, class: Class, via: u32) -> Reference {
    Reference {
        node: Some(named(1, name)),
        name: text.to_owned(),
        display: text.to_owned(),
        class,
        via: Some(ns0(via)),
    }
}

/// The names of the variables of `Plant/Types`, in the order of their ids.
const TYPES: [&str; 15] = [
    "Boolean",
    "SByte",
    "Byte",
    "Int16",
    "UInt16",
    "Int32",
    "UInt32",
    "Int64",
    "UInt64",
    "Float",
    "Double",
    "String",
    "DateTime",
    "Guid",
    "ByteString",
];

const ORGANIZES: u32 = 35;
const HAS_COMPONENT: u32 = 47;

/// Each page of a browse of `node`, which follows each continuation point.
async fn pages(client: &mut Client, node: &Node) -> Vec<Page> {
    let most = NonZeroU32::MAX;
    let mut pages = vec![
        client
            .browse(node, most)
            .await
            .expect("a browse")
            .expect("a node"),
    ];
    while let Some(point) = pages.last_mut().and_then(|page| page.next.take()) {
        pages.push(client.browse_next(point).await.expect("a browse next"));
    }
    pages
}

/// The references of each page of `pages`.
fn references(pages: Vec<Page>) -> Vec<Vec<Reference>> {
    pages.into_iter().map(|page| page.references).collect()
}

#[test]
fn a_client_browses_the_tree_from_objects() {
    let (objects, plant, line, types, odd) = with_tree(async |client| {
        let objects = pages(client, &ns0(85)).await;
        let plant = pages(client, &named(1, "Plant")).await;
        let line = pages(client, &named(1, "Plant.Line")).await;
        let types = pages(client, &named(1, "Plant.Types")).await;
        let odd = pages(client, &named(1, "Plant.Odd")).await;
        (objects, plant, line, types, odd)
    });
    let plant_folder = reference("Plant", "Plant", Class::Object, ORGANIZES);
    let objects: Vec<Reference> = objects
        .into_iter()
        .flat_map(|page| page.references)
        .collect();
    assert!(objects.contains(&plant_folder), "{objects:?}");
    assert_eq!(
        references(plant),
        [[
            reference("Plant.Types", "Types", Class::Object, ORGANIZES),
            reference("Plant.Odd", "Odd", Class::Object, ORGANIZES),
            reference("Plant.Line", "Line", Class::Object, HAS_COMPONENT),
        ]]
    );
    let variable = |name: &str| {
        let node = format!("Plant.Line.{name}");
        reference(&node, name, Class::Variable, HAS_COMPONENT)
    };
    assert_eq!(
        references(line),
        [vec![
            plant_folder,
            variable("Temperature"),
            variable("Speed"),
            variable("Mode"),
            variable("Profile"),
            variable("Locked"),
            variable("Recipe"),
        ]]
    );
    let types: Vec<Vec<Reference>> = references(types);
    let expected: Vec<Reference> = (TYPES.iter())
        .map(|name| {
            let node = format!("Plant.Types.{name}");
            reference(&node, name, Class::Variable, HAS_COMPONENT)
        })
        .collect();
    let mut first = expected;
    let second = first.split_off(8);
    assert_eq!(types, [first, second]);
    let level = |via| reference("Plant.Odd.Level", "Level", Class::Variable, via);
    assert_eq!(
        references(odd),
        [[
            Reference {
                node: None,
                ..reference("Guid", "Guid", Class::Object, ORGANIZES)
            },
            level(ORGANIZES),
            level(HAS_COMPONENT),
            Reference {
                node: Some(named(1, "Plant.Odd.Twin")),
                ..level(HAS_COMPONENT)
            },
        ]]
    );
}

#[test]
fn a_client_describes_each_variable_of_the_tree() {
    let line = [
        "Temperature",
        "Speed",
        "Mode",
        "Profile",
        "Locked",
        "Recipe",
    ];
    let nodes: Vec<Node> = (TYPES.iter().map(|name| format!("Plant.Types.{name}")))
        .chain(line.iter().map(|name| format!("Plant.Line.{name}")))
        .map(|name| named(1, &name))
        .collect();
    let described = with_tree(async move |client| client.describe(&nodes).await);
    let description = |base: Option<u32>, rank: i32, access: u8| Description {
        base,
        rank: Some(rank),
        access: Some(access),
        unit: None,
    };
    let mut expected: Vec<Description> = (1..=15)
        .map(|base| description(Some(base), -1, 1))
        .collect();
    expected.extend([
        Description {
            unit: Some(Unit {
                text: Some("°C".to_owned()),
                code: Some("CEL".to_owned()),
            }),
            ..description(Some(11), -1, 1)
        },
        description(Some(11), -1, 1),
        description(Some(6), -1, 1),
        description(Some(11), 1, 1),
        description(Some(11), -1, 0),
        description(None, -1, 1),
    ]);
    assert_eq!(described, Ok(expected));
}

#[test]
fn a_read_of_each_variable_of_the_tree_gives_its_value() {
    let names: Vec<String> = (TYPES.iter().map(|name| format!("Plant.Types.{name}")))
        .chain(
            [
                "Temperature",
                "Speed",
                "Mode",
                "Profile",
                "Locked",
                "Recipe",
            ]
            .map(|name| format!("Plant.Line.{name}")),
        )
        .chain(["Plant.Odd.Level".to_owned(), "Plant.Odd.Twin".to_owned()])
        .collect();
    let reads = with_tree(async move |client| {
        let mut reads = Vec::new();
        for name in &names {
            reads.push(client.read_one(&named(1, name)).await);
        }
        reads
    });
    let mistyped = |name: &str| Err(Fault::of_type(Type::named(name)));
    assert_eq!(
        reads,
        [
            Ok(Value::Bool(true)),
            Ok(Value::I8(-8)),
            Ok(Value::U8(8)),
            Ok(Value::I16(-16)),
            Ok(Value::U16(16)),
            Ok(Value::I32(-32)),
            Ok(Value::U32(32)),
            Ok(Value::I64(-64)),
            Ok(Value::U64(64)),
            Ok(Value::F32(0.5)),
            Ok(Value::F64(0.25)),
            mistyped("String"),
            mistyped("DateTime"),
            mistyped("Guid"),
            mistyped("ByteString"),
            Ok(Value::F64(21.5)),
            Ok(Value::F64(1.25)),
            Ok(Value::I32(0)),
            mistyped("array of Double"),
            Err(Fault::status(BAD_USER_ACCESS_DENIED)),
            mistyped("ExtensionObject"),
            Ok(Value::F64(2.5)),
            Ok(Value::F64(3.5)),
        ]
    );
}

#[test]
fn a_server_panics_on_a_variable_of_a_node_of_the_tree() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&["ns=1;s=Plant.Line.Speed"]);
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listen(&node),
            &mut rng,
            &variables,
            async {},
        )
        .await;
    });
    let Err(::sim::Error::Panicked { message, .. }) = run else {
        panic!("the serve panics: {run:?}");
    };
    assert!(
        message.starts_with(
            "assertion `left == right` failed: open62541 refused the node"
        ),
        "{message}"
    );
}

async fn connect(
    node: &node::Node,
    rng: &mut env::rng::Rng,
    url: &CStr,
) -> Result<Client, Status> {
    let time = event::Time::monotonic(node.clock());
    let connect = Client::connect(time, node.net(), rng, url, future::pending());
    Ok(connect.await?.expect("a pending wait never ends"))
}
