//! The conformance suite of the `opcua` kind.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

use std::net::SocketAddr;
use std::num::NonZeroUsize;

use connector::kind::Table;
use connector_opcua::Kind;
use connector_opcua::sim::{Variables, serve};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Attribute, Block, Document, Label, Map};
use env::net::tcp;
use spec::channel::{Channel, Data};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::channel;
use types::name::Name;
use types::sample::{Scalar, Type};

#[path = "../scenarios.rs"]
mod scenarios;

use scenarios::{Backoff, Plant, Serve, Suite};

/// The variables of the server, each read into the channel of the same place in
/// [`CHANNELS`].
const NODES: [&str; 3] = ["ns=3;s=SpikeData", "ns=3;i=7", "ns=2;s=Dip"];
/// The data channels, on `plant.time`.
const CHANNELS: [&str; 3] = ["plant.a", "plant.b", "plant.c"];

const SUITE: Suite = Suite {
    kinds,
    setup: |plant| Box::pin(std::future::ready(setup(&plant))),
    serve: |inputs| Box::pin(simulate(inputs)),
    progress: "in",
    backoff: Backoff::Restart,
    refused,
};

scenarios::suite!(SUITE);

fn kinds() -> Table {
    Table::new().with("opcua", Kind::default())
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// Defines `plant.time` and the `f64` channels of [`CHANNELS`] on it, and gives the
/// config of a connector that reads each of them at 10 Hz.
fn setup(plant: &Plant<'_>) -> Document {
    let time = channel::Key::from_u128(1);
    let index = spec::channel::Kind::Index {
        error: None,
        control: None,
    };
    let mut definitions = vec![(
        name("plant.time"),
        Definition::Channel(Channel {
            key: time,
            kind: index,
        }),
    )];
    for (key, channel) in (2..).zip(CHANNELS) {
        let sample = DataType::Sample(Type::Scalar(Scalar::F64));
        let data = Data::new(time, None, sample, None).expect("no unit");
        let kind = spec::channel::Kind::Data(data);
        let key = channel::Key::from_u128(key);
        definitions.push((name(channel), Definition::Channel(Channel { key, kind })));
    }
    plant.rig.define(definitions);
    let address = format!("opc.tcp://{}", plant.address);
    let reads = CHANNELS
        .iter()
        .zip(NODES)
        .map(|(&channel, node)| (channel, node));
    config(&address, "10Hz", reads.collect())
}

/// Serves the variables of [`NODES`] until `until`.
async fn simulate(inputs: Serve) {
    let listen = tcp::Listen {
        local: inputs.address,
        backlog: 4,
        options: tcp::Options {
            send_buffer_bytes: 1 << 16,
            recv_buffer_bytes: 1 << 16,
            unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
            delayed: false,
        },
    };
    let net = inputs.node.net();
    let listener = net.listen(&listen).expect("the port is free");
    let wall = inputs.node.wall();
    #[expect(
        clippy::disallowed_methods,
        reason = "a simulated device reads the wall time of its node"
    )]
    let now = move || wall.now().time;
    let mut rng = inputs.node.entropy().rng();
    let (clock, variables) = (inputs.node.clock(), Variables::new(&NODES));
    let served = serve(
        clock,
        now,
        net,
        listener,
        &mut rng,
        &variables,
        inputs.until,
    );
    served.await.expect("the listener holds");
}

/// Configs with a missing scheme, no `read` block, and a node id that does not parse.
fn refused() -> Vec<(Document, Vec<Diagnostic>)> {
    let device = SocketAddr::from(([10, 0, 0, 2], 9000));
    let read = vec![("plant.a", NODES[0])];
    vec![
        (
            config(&device.to_string(), "10Hz", read),
            vec![diagnostic(
                "opcua.bad-address",
                "the address \"10.0.0.2:9000\" does not start with opc.tcp://",
                "Write an address such as \"opc.tcp://plc:4840\"",
            )],
        ),
        (
            config("opc.tcp://plc:4840", "10Hz", Vec::new()),
            vec![diagnostic(
                "opcua.no-read",
                "the connector has no `read` block",
                "Add a `read` block for each channel, such as \
                 `read \"plant.spike\" { node_id = \"ns=3;s=SpikeData\" }`",
            )],
        ),
        (
            config("opc.tcp://plc:4840", "10Hz", vec![("plant.a", "nope")]),
            vec![diagnostic(
                "opcua.bad-node-id",
                "a node ID is ns=<index>;i=<number> or ns=<index>;s=<name>, with an \
                 index to 65535, a number to 4294967295, and a name that is not empty",
                "Write a node ID such as \"ns=3;s=SpikeData\" or \"ns=3;i=1001\"",
            )],
        ),
    ]
}

fn attribute(key: &str, text: &str) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value: Value {
            kind: value::Kind::String(text.into()),
            span: None,
        },
    }
}

/// The config of a connector of `address` at `rate`, with a `read` block for each
/// channel and node of `reads`.
fn config(address: &str, rate: &str, reads: Vec<(&str, &str)>) -> Document {
    let attributes = vec![attribute("address", address), attribute("rate", rate)];
    let blocks = reads.into_iter().map(|(channel, node)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![Label {
            text: channel.into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![attribute("node_id", node)]).expect("one key"),
            blocks: Vec::new(),
        },
        span: None,
    });
    Document {
        attributes: Map::new(attributes).expect("unique keys"),
        blocks: blocks.collect(),
    }
}

fn diagnostic(code: &'static str, message: &str, fix: &str) -> Diagnostic {
    Diagnostic::new(Code::new(code), None, message.into(), fix.into())
}
