use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use connector::cancel;
use connector::kind::{self, Channels, Context, Table};
use connector::supervisor::{self, Supervisor};
use document::Document;
use document::diagnostic::Diagnostic;
use serde_json::{Value, json};
use spec::definition::Definition;
use types::channel::Key;
use types::time::Span;

use super::{Class, Failure, Output, State, Status};
use crate::Node;
use crate::common::{Fixture, SAM, founded, front_ends, name, solo};
use crate::serve::serve;

/// A kind whose run sets the count `in` to 12 and `confirmed` to 10, then waits for
/// its cancel.
struct Steady;

/// A kind whose run ends with a device error.
struct Failing;

/// The counts that both kinds name.
fn counts() -> Channels {
    Channels {
        reader: None,
        writes: Vec::new(),
        counts: vec![name("in"), name("confirmed")],
    }
}

impl kind::Kind for Steady {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(counts())
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, kind::Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn run(&self, context: Context<()>) -> Result<(), kind::Error> {
        context.count("in").set(12);
        context.count("confirmed").set(10);
        context.cancel().wait().await;
        Ok(())
    }
}

impl kind::Kind for Failing {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(counts())
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, kind::Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn run(&self, _: Context<()>) -> impl Future<Output = Result<(), kind::Error>> {
        std::future::ready(Err(kind::Error::Device("no route to the device".into())))
    }
}

fn kinds() -> Table {
    Table::new()
        .with("steady", Steady)
        .with("failing", Failing)
        .with("forging", Forging)
}

/// A kind whose run ends with a device error whose text, from the device, holds a
/// newline.
struct Forging;

impl kind::Kind for Forging {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(counts())
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, kind::Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn run(&self, _: Context<()>) -> impl Future<Output = Result<(), kind::Error>> {
        std::future::ready(Err(kind::Error::Device(
            "refused\nsite.a: config error: forged by the device".into(),
        )))
    }
}

#[test]
fn escapes_a_newline_in_the_text_of_an_error() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "forging", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        supervise(&fixture, &node, &["site.a", "site.b"]).await;
        let (head, _) = status(&node, "@admin").await;
        let output: Output =
            serde_json::from_value(head["output"].clone()).expect("an output");
        let text = output.text();
        let (_, errors) = text.split_once("\n\n").expect("a line of errors");
        let line = "site.b: device error: refused\\nsite.a: config error: forged by \
                    the device\n";
        assert_eq!(errors, line);
    });
}

/// The node of `mesh` on `fixture`, with the kinds of [`kinds`].
fn create_node(fixture: &Fixture, mesh: mesh::Mesh) -> Node {
    let made = Cell::new(0);
    let key = move || {
        made.set(made.get() + 1);
        Key::from_u128(made.get())
    };
    Node::new(
        mesh,
        key,
        front_ends(),
        Arc::new(kinds()),
        fixture.hub.clone(),
        fixture.sim.clock(),
        fixture.name.clone(),
    )
}

/// The text of a connector `name` of `kind` on the node `node`, and a placement that
/// homes its indexes on `node`.
fn connector(name: &str, kind: &str, node: &str) -> String {
    format!(
        "connector \"{name}\" {{\n  kind = \"{kind}\"\n  node = \"{node}\"\n  \
         address = \"opc.tcp://{name}:4840\"\n}}\nplacement \"{name}.home\" {{\n  \
         select = \"{name}.**\"\n  home = \"{node}\"\n}}\n"
    )
}

/// Applies `text` on `node`, and defines its spec on the hub of `fixture`.
async fn applied(fixture: &Fixture, node: &Node, text: &str) {
    let files = vec![(PathBuf::from("site.hcl"), text.to_owned())];
    let (plan, _) = node.plan(files).await.expect("a plan");
    node.apply(Path::new("site.plan"), &plan)
        .await
        .expect("an apply");
    let spec = node.mesh().spec().await.expect("a spec");
    fixture.hub.set_definitions(spec.definitions.iter());
}

/// Runs a supervisor of each connector `names` of the spec of `node`, as the node
/// does, until the run ends.
async fn supervise(fixture: &Fixture, node: &Node, names: &[&str]) {
    let supervisor = Supervisor::new(supervisor::Config {
        kinds: Arc::new(kinds()),
        clock: fixture.sim.clock(),
        entropy: fixture.sim.entropy(),
        net: fixture.sim.net(),
        tasks: fixture.tasks.clone(),
        hub: fixture.hub.clone(),
    });
    let spec = node.mesh().spec().await.expect("a spec");
    for connector in names {
        let Some(Definition::Connector(placed)) =
            spec.definitions.get(&name(connector))
        else {
            panic!("the spec holds the connector {connector}");
        };
        let (kind, config) = (placed.kind().to_string(), placed.config().document());
        let (supervisor, config, connector) =
            (supervisor.clone(), config.clone(), name(connector));
        fixture.tasks.spawn(async move {
            let cancel = cancel::Token::new();
            let opened = supervisor.open(&kind, connector, &config, &cancel).await;
            opened
                .run()
                .await
                .expect("a run that ends only at a cancel");
        });
    }
    fixture
        .sim
        .clock()
        .sleep(Span::from_nanos(3 * Span::SECOND.nanos()))
        .await;
}

/// The first line of the reply of `status` on `node` as `subject`, and the bytes
/// after it.
async fn status(node: &Node, subject: &str) -> (Value, Vec<u8>) {
    let reply = serve(node, &name(subject), b"{\"status\":{}}\n").await;
    let end = reply
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("a line");
    let head = serde_json::from_slice(&reply[..end]).expect("a line of JSON");
    (head, reply[end + 1..].to_vec())
}

/// The status of `name` of `kind` with no frame of its status.
fn unknown(name: &str, kind: &str) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "state": "unknown",
        "address": format!("opc.tcp://{name}:4840"),
        "restarts": null,
        "counts": null,
        "error": null,
    })
}

#[test]
fn gives_the_state_counts_and_error_of_each_connector_on_the_node() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "failing", "edge"),
            connector("site.c", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        supervise(&fixture, &node, &["site.a", "site.b"]).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({ "output": { "connectors": [
            {
                "name": "site.a",
                "kind": "steady",
                "state": "running",
                "address": "opc.tcp://site.a:4840",
                "restarts": 0,
                "counts": { "confirmed": 10, "in": 12 },
                "error": null,
            },
            {
                "name": "site.b",
                "kind": "failing",
                "state": "restarting",
                "address": "opc.tcp://site.b:4840",
                "restarts": 3,
                "counts": { "confirmed": 0, "in": 0 },
                "error": { "class": "device", "text": "no route to the device" },
            },
            unknown("site.c", "steady"),
        ]}});
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn waits_one_second_in_all_for_connectors_that_no_supervisor_started() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let names = ["site.a", "site.b", "site.c"];
        let text: Vec<_> = names
            .iter()
            .map(|name| connector(name, "steady", "edge"))
            .collect();
        applied(&fixture, &node, &text.concat()).await;
        let clock = fixture.sim.clock();
        let start = clock.now();
        let (head, _) = status(&node, "@admin").await;
        assert_eq!(clock.now() - start, Span::SECOND);
        let connectors: Vec<_> =
            names.iter().map(|name| unknown(name, "steady")).collect();
        assert_eq!(head, json!({ "output": { "connectors": connectors } }));
    });
}

#[test]
fn replies_once_each_connector_gave_its_frame() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        supervise(&fixture, &node, &["site.a", "site.b"]).await;
        let clock = fixture.sim.clock();
        let start = clock.now();
        let (head, _) = status(&node, "@admin").await;
        let waited = clock.now() - start;
        assert!(waited < Span::MILLISECOND, "waited {waited:?}");
        let states = head["output"]["connectors"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|status| status["state"].clone())
            .collect::<Vec<_>>();
        assert_eq!(states, [json!("running"), json!("running")]);
    });
}

#[test]
fn leaves_out_each_connector_on_another_node() {
    let definitions = spec::founding::create(crate::common::ADMIN.public());
    founded(&["edge", "far"], definitions, |fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "steady", "far"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        let (head, _) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
    });
}

/// The head of the refusal of `subject`, which reads no status channel.
fn refused(subject: &str) -> Value {
    let message = format!(
        "no access policy gives `{subject}` the action `read` on a request that names \
         no definition"
    );
    json!({
        "errors": [{
            "code": "ops.denied",
            "message": message,
            "fix": "Ask an admin of the region for an access policy that allows it",
            "notes": [],
        }],
        "status": 1,
    })
}

#[test]
fn refuses_an_admin_on_a_node_that_places_no_connector() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, "").await;
        let (head, rest) = status(&node, "@admin").await;
        assert_eq!((head, rest), (refused("@admin"), Vec::new()));
    });
}

#[test]
fn refuses_an_admin_on_a_node_that_uses_no_spec() {
    let mut founding = spec::founding::create(crate::common::ADMIN.public());
    let policy = founding[&name("@admin.@access")].clone();
    // An access policy at a key that is no tree key of its kind: a problem.
    founding.insert(name("x"), policy);
    founded(&["edge"], founding, |fixture, mesh| async move {
        let node = create_node(&fixture, mesh.clone());
        let spec = mesh.spec().await.expect("a spec");
        assert_eq!(spec.pointer, None, "{spec:?}");
        let (head, rest) = status(&node, "@admin").await;
        assert_eq!((head, rest), (refused("@admin"), Vec::new()));
    });
}

/// `sam` may read the names under `site.a`.
fn reader() -> String {
    format!(
        "subject \"sam\" {{ keys = [\"{SAM}\"] }}\naccess \"watch\" {{\n  subjects = \
         \"sam\"\n  select = \"site.a.**\"\n  allow = [read]\n}}\n"
    )
}

#[test]
fn leaves_out_each_connector_whose_status_the_subject_may_not_read() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            reader(),
            connector("site.a", "steady", "edge"),
            connector("site.b", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        let (head, _) = status(&node, "sam").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
    });
}

#[test]
fn refuses_a_subject_that_may_read_no_connector_on_the_node() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [reader(), connector("site.b", "steady", "edge")];
        applied(&fixture, &node, &text.concat()).await;
        let (head, rest) = status(&node, "sam").await;
        assert_eq!(head, refused("sam"));
        assert_eq!(rest, b"");
    });
}

#[test]
fn refuses_a_subject_that_may_read_only_some_status_channels() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            format!(
                "subject \"sam\" {{ keys = [\"{SAM}\"] }}\naccess \"watch\" {{\n  \
                 subjects = \"sam\"\n  select = \"site.a.status.state\"\n  allow = \
                 [read]\n}}\n"
            ),
            connector("site.a", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, _) = status(&node, "sam").await;
        assert_eq!(head, refused("sam"));
    });
}

#[test]
fn fails_when_the_hub_does_not_define_the_status_channels() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = connector("site.a", "steady", "edge");
        let files = vec![(PathBuf::from("site.hcl"), text)];
        let (plan, _) = node.plan(files).await.expect("a plan");
        node.apply(Path::new("site.plan"), &plan)
            .await
            .expect("an apply");
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({
            "errors": [{
                "code": "ops.channel-read",
                "message": "the status channels of the connector `site.a` did not \
                            read: the selector matches no channel",
                "fix": "Fix the cause in the message, then ask for the status again",
                "notes": [],
            }],
            "status": 1,
        });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn refuses_bytes_after_the_line_of_status() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let reply = serve(&node, &name("@admin"), b"{\"status\":{}}\nmore").await;
        let expected = json!({
            "errors": [{
                "code": "ops.bad-request",
                "message": "the body is no request: bytes follow the first line of \
                            `status`",
                "fix": "Use a `foundation` program of the same version as the node",
                "notes": [],
            }],
            "status": 1,
        });
        let head: Value =
            serde_json::from_slice(reply.strip_suffix(b"\n").expect("a line"))
                .expect("JSON");
        assert_eq!(head, expected);
    });
}

/// A status of `name` in `state`, with the counts `in` and `confirmed` and `error`.
fn line(
    name: &str,
    state: State,
    counts: Option<(u64, u64)>,
    error: Option<Failure>,
) -> Status {
    Status {
        name: name.to_owned(),
        kind: "opcua".to_owned(),
        state,
        address: Some(format!("opc.tcp://{name}:4840")),
        restarts: counts.map(|_| 0),
        counts: counts.map(|(n, confirmed)| {
            BTreeMap::from([("in".to_owned(), n), ("confirmed".to_owned(), confirmed)])
        }),
        error,
    }
}

#[test]
fn writes_a_table_then_the_line_of_each_error() {
    let failure = Failure {
        class: Class::Device,
        text: "no route to the device".to_owned(),
    };
    let output = Output {
        connectors: vec![
            line("plant.plc", State::Running, Some((1200, 1150)), None),
            line(
                "plant.press",
                State::Restarting,
                Some((0, 0)),
                Some(failure),
            ),
            Status {
                address: None,
                ..line("yard.meter", State::Unknown, None, None)
            },
        ],
    };
    let expected = "\
CONNECTOR    KIND   STATE       ADDRESS                     IN    CONFIRMED
plant.plc    opcua  running     opc.tcp://plant.plc:4840    1200  1150
plant.press  opcua  restarting  opc.tcp://plant.press:4840  0     0
yard.meter   opcua  unknown     -                           -     -

plant.press: device error: no route to the device
";
    assert_eq!(output.text(), expected);
}

#[test]
fn writes_each_class_and_state_as_its_word() {
    let states = [
        (State::Running, "running"),
        (State::Restarting, "restarting"),
        (State::Stopped, "stopped"),
        (State::Ending, "ending"),
        (State::Unknown, "unknown"),
    ];
    for (state, word) in states {
        assert_eq!(state.word(), word);
        assert_eq!(serde_json::to_value(state).expect("JSON"), json!(word));
    }
    for (class, word) in [
        (Class::Config, "config"),
        (Class::Device, "device"),
        (Class::Retry, "retry"),
    ] {
        assert_eq!(class.word(), word);
        assert_eq!(serde_json::to_value(class).expect("JSON"), json!(word));
    }
}

#[test]
fn writes_no_table_with_no_connector() {
    let output = Output {
        connectors: Vec::new(),
    };
    assert_eq!(output.text(), "");
}
