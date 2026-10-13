use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use connector::cancel;
use connector::kind::{self, Channels, Context, Table};
use connector::supervisor::{self, Supervisor};
use document::Document;
use document::diagnostic::Diagnostic;
use env::paths::Paths;
use serde_json::{Value, json};
use spec::definition::Definition;
use types::authority::Authority;
use types::channel::Key;
use types::frame::{Form, Label, Path as Stream};
use types::name::Name;
use types::sample::Type;
use types::time::Span;

use super::{Class, Failure, Output, State, Status};
use crate::common::{self, Fixture, SAM, founded, front_ends, name, solo};
use crate::serve::serve;
use crate::{Failure as Lost, Node, Run};

/// A kind that names the counts `counts`, and whose run sets each to its value, then
/// waits for its cancel, or ends at once, as `script` says.
struct Scripted {
    counts: &'static [(&'static str, u64)],
    script: Script,
    failed: AtomicBool,
}

/// What a run of [`Scripted`] does with an error of the text it holds.
#[derive(Clone, Copy)]
enum Script {
    Steady,
    /// Each run ends at once with a device error.
    Fails(&'static str),
    /// Each run shows the error.
    Shows(&'static str),
    /// The first run ends at once with a device error, and each later run clears it.
    Clears(&'static str),
}

impl Scripted {
    fn new(counts: &'static [(&'static str, u64)], script: Script) -> Self {
        Self {
            counts,
            script,
            failed: AtomicBool::new(false),
        }
    }
}

impl kind::Kind for Scripted {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels {
            reader: None,
            writes: Vec::new(),
            counts: self.counts.iter().map(|(count, _)| name(count)).collect(),
        })
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, kind::Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn run(&self, context: Context<()>) -> Result<(), kind::Error> {
        match self.script {
            Script::Steady => {}
            Script::Fails(error) => return Err(kind::Error::Device(error.into())),
            Script::Shows(error) => context.error(&error),
            Script::Clears(error) => {
                if !self.failed.swap(true, Ordering::Relaxed) {
                    return Err(kind::Error::Device(error.into()));
                }
                context.clear_error();
            }
        }
        for (count, value) in self.counts {
            context.count(count).set(*value);
        }
        context.cancel().wait().await;
        Ok(())
    }
}

/// `steady` sets `in` to 12 and `confirmed` to 10, `failing` ends with a device error,
/// `forging` with one whose text, from the device, holds a newline, `showing` shows an
/// error, `clearing` clears the error of its failed first run, `wide` also names
/// `extra`, and `shifted` names `in` and `other`.
fn kinds() -> Table {
    let counts = &[("in", 12), ("confirmed", 10)];
    let failing = "no route to the device";
    let forging = "refused\nsite.a: config error: forged by the device";
    let wide = &[("in", 0), ("confirmed", 0), ("extra", 0)];
    Table::new()
        .with("steady", Scripted::new(counts, Script::Steady))
        .with("failing", Scripted::new(counts, Script::Fails(failing)))
        .with("forging", Scripted::new(counts, Script::Fails(forging)))
        .with("showing", Scripted::new(counts, Script::Shows("busy")))
        .with("clearing", Scripted::new(counts, Script::Clears(failing)))
        .with("wide", Scripted::new(wide, Script::Steady))
        .with(
            "shifted",
            Scripted::new(&[("in", 0), ("other", 0)], Script::Steady),
        )
}

#[test]
fn gives_unknown_while_the_hub_lacks_one_status_channel_of_the_spec() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "steady", "edge")).await;
        supervise(&fixture, &node, &["site.a"]).await;
        applied_alone(&node, &connector("site.a", "wide", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "wide")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_unknown_until_the_run_of_a_spec_with_more_counts_writes_its_start_frame() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "steady", "edge")).await;
        let steady = supervise(&fixture, &node, &["site.a"]).await;
        applied(&fixture, &node, &connector("site.a", "wide", "edge")).await;
        steady.cancel();
        let (head, rest, waited) = timed_status(&fixture, &node, "@admin").await;
        assert!(
            waited < Span::SECOND,
            "the reply waited {waited} for the frame, not under 1 s"
        );
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "wide")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({ "output": { "connectors": [{
            "name": "site.a",
            "kind": "wide",
            "state": "running",
            "address": "opc.tcp://site.a:4840",
            "restarts": 0,
            "counts": { "confirmed": 0, "extra": 0, "in": 0 },
            "error": null,
        }]}});
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_unknown_until_the_run_of_a_spec_with_another_count_writes_its_start_frame() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "wide", "edge")).await;
        let wide = supervise(&fixture, &node, &["site.a"]).await;
        applied(&fixture, &node, &connector("site.a", "shifted", "edge")).await;
        wide.cancel();
        let (head, rest, waited) = timed_status(&fixture, &node, "@admin").await;
        assert!(
            waited < Span::SECOND,
            "the reply waited {waited} for the frame, not under 1 s"
        );
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "shifted")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({ "output": { "connectors": [{
            "name": "site.a",
            "kind": "shifted",
            "state": "running",
            "address": "opc.tcp://site.a:4840",
            "restarts": 0,
            "counts": { "in": 0, "other": 0 },
            "error": null,
        }]}});
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_the_old_state_until_the_run_with_fewer_counts_writes_its_start_frame() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "wide", "edge")).await;
        let wide = supervise(&fixture, &node, &["site.a"]).await;
        applied(&fixture, &node, &connector("site.a", "steady", "edge")).await;
        wide.cancel();
        let running = |counts| {
            json!({ "output": { "connectors": [{
                "name": "site.a",
                "kind": "steady",
                "state": "running",
                "address": "opc.tcp://site.a:4840",
                "restarts": 0,
                "counts": counts,
                "error": null,
            }]}})
        };
        let (head, rest) = status(&node, "@admin").await;
        assert_eq!(head, running(json!({ "confirmed": 0, "in": 0 })));
        assert_eq!(rest, b"");
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, rest) = status(&node, "@admin").await;
        assert_eq!(head, running(json!({ "confirmed": 10, "in": 12 })));
        assert_eq!(rest, b"");
    });
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

/// Applies `text` on `node`, with no change to the hub of the node.
async fn applied_alone(node: &Node, text: &str) {
    let files = vec![(PathBuf::from("site.hcl"), text.to_owned())];
    let (plan, _) = node.plan(files).await.expect("a plan");
    node.apply(Path::new("site.plan"), &plan)
        .await
        .expect("an apply");
}

/// Runs a supervisor of each connector `names` of the spec of `node`, as the node
/// does, until the run ends. Gives the token that cancels each run.
async fn supervise(fixture: &Fixture, node: &Node, names: &[&str]) -> cancel::Token {
    let supervisor = Supervisor::new(supervisor::Config {
        kinds: Arc::new(kinds()),
        clock: fixture.sim.clock(),
        entropy: fixture.sim.entropy(),
        net: fixture.sim.net(),
        tasks: fixture.tasks.clone(),
        hub: fixture.hub.clone(),
        secrets: Box::new(secret::store::File::new(
            fixture.sim.files(),
            std::path::Path::new("secrets"),
        )),
    });
    let spec = node.mesh().spec().await.expect("a spec");
    let cancel = cancel::Token::new();
    for connector in names {
        let Some(Definition::Connector(placed)) =
            spec.definitions.get(&name(connector))
        else {
            panic!("the spec holds the connector {connector}");
        };
        let (kind, config) = (placed.kind().to_string(), placed.config().document());
        let (supervisor, config, connector, cancel) = (
            supervisor.clone(),
            config.clone(),
            name(connector),
            cancel.clone(),
        );
        fixture.tasks.spawn(async move {
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
    cancel
}

/// Writes one frame of the channel `index` and of each channel `name` on it with the
/// sample `bytes`, as a program.
async fn write(fixture: &Fixture, index: &str, samples: &[(&str, &[u8])]) {
    let names = std::iter::once(index).chain(samples.iter().map(|(name, _)| *name));
    let channels = names.map(name);
    let config = hub::writer::Config {
        subject: name("program"),
        authority: Authority(1),
        lease: None,
        channels: channels.collect(),
    };
    let mut program = fixture.hub.writer(config).await.expect("the program opens");
    let entries = program.entries().to_vec();
    let set = Arc::clone(program.set());
    let stamp = u64::try_from(program.now().nanos()).expect("a stamp after 1970");
    let stamp = stamp.to_le_bytes();
    let values =
        std::iter::once(&stamp[..]).chain(samples.iter().map(|(_, bytes)| *bytes));
    let bytes: Vec<(usize, &[u8])> = entries.iter().copied().zip(values).collect();
    let mut series: Vec<_> = bytes
        .iter()
        .map(|(entry, bytes)| (*entry, bytes.len()))
        .collect();
    series.sort_unstable();
    let mut draft = program.draft(Form::Raw, &series).expect("a draft");
    for (entry, bytes) in &bytes {
        let series = draft.series_mut(*entry).expect("a series of the draft");
        series.copy_from_slice(bytes);
    }
    draft.set_count(set.entries()[entries[0]].group, 1);
    program
        .write(Label::Path(Stream::Live), draft)
        .expect("the write goes");
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

/// The reply of [`status`], and the time of the simulated clock that it waited.
async fn timed_status(
    fixture: &Fixture,
    node: &Node,
    subject: &str,
) -> (Value, Vec<u8>, Span) {
    let clock = fixture.sim.clock();
    let start = clock.now();
    let (head, rest) = status(node, subject).await;
    (head, rest, clock.now() - start)
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

/// The run, output, and errors of `foundation <args>` on `node`, which serves each
/// request as `@admin`.
async fn cli(node: &Node, paths: &Paths, args: &[&str]) -> (Run, String, String) {
    let reach = async |_: &Path| {
        Ok::<_, Lost>(async |body: &[u8]| {
            Ok::<_, Lost>(serve(node, &name("@admin"), body).await)
        })
    };
    common::cli(paths, args, reach).await
}

#[test]
fn the_command_line_writes_the_status_of_the_node_as_text_or_json() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "failing", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        supervise(&fixture, &node, &["site.a", "site.b"]).await;
        let paths = fixture.sim.paths();
        let table = "\
CONNECTOR  KIND     STATE       ADDRESS                IN  CONFIRMED
site.a     steady   running     opc.tcp://site.a:4840  12  10
site.b     failing  restarting  opc.tcp://site.b:4840  0   0

site.b: device error: no route to the device
";
        let shown = cli(&node, &paths, &["status"]).await;
        assert_eq!(shown, (Run::Exit(0), table.to_owned(), String::new()));
        let (run, json, errors) = cli(&node, &paths, &["status", "--json"]).await;
        assert_eq!((run, errors.as_str()), (Run::Exit(0), ""));
        let output: Value = serde_json::from_str(&json).expect("JSON");
        assert_eq!(output["connectors"][1]["error"]["class"], "device");
        let (head, _) = status(&node, "@admin").await;
        assert_eq!(json, format!("{}\n", head["output"]));
    });
}

#[test]
fn the_command_line_writes_the_refusal_of_status() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, "").await;
        let (run, output, errors) = cli(&node, &fixture.sim.paths(), &["status"]).await;
        let refused = "error[ops.denied]: no access policy gives `@admin` the action \
                       `read` on a request that names no definition\n\
                       fix: Ask an admin of the region for an access policy that \
                       allows it\n";
        assert_eq!(
            (run, output.as_str(), errors.as_str()),
            (Run::Exit(1), "", refused)
        );
    });
}

#[test]
fn gives_the_error_that_a_running_kind_shows() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "showing", "edge")).await;
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({ "output": { "connectors": [{
            "name": "site.a",
            "kind": "showing",
            "state": "running",
            "address": "opc.tcp://site.a:4840",
            "restarts": 0,
            "counts": { "confirmed": 10, "in": 12 },
            "error": { "class": "retry", "text": "busy" },
        }]}});
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_no_error_once_the_running_kind_clears_the_error_of_a_failed_run() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "clearing", "edge")).await;
        supervise(&fixture, &node, &["site.a"]).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected = json!({ "output": { "connectors": [{
            "name": "site.a",
            "kind": "clearing",
            "state": "running",
            "address": "opc.tcp://site.a:4840",
            "restarts": 1,
            "counts": { "confirmed": 10, "in": 12 },
            "error": null,
        }]}});
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
        let (head, _, waited) = timed_status(&fixture, &node, "@admin").await;
        assert_eq!(waited, Span::SECOND);
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
        let (head, _, waited) = timed_status(&fixture, &node, "@admin").await;
        assert!(
            waited < Span::MILLISECOND,
            "the reply waited {waited}, not under 1 ms"
        );
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
fn gives_unknown_for_a_frame_with_no_status() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "steady", "edge")).await;
        write_user(&fixture, "site.a.status.time", &[9]).await;
        let (head, _, waited) = timed_status(&fixture, &node, "@admin").await;
        assert!(
            waited < Span::SECOND,
            "the reply waited {waited} for the frame, not under 1 s"
        );
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
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
fn gives_unknown_when_the_hub_does_not_define_the_status_channels_yet() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied_alone(&node, &connector("site.a", "steady", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_unknown_for_a_connector_whose_channels_go_during_the_read() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        let spec = node.mesh().spec().await.expect("a spec");
        let kept: BTreeMap<_, _> = spec
            .definitions
            .iter()
            .filter(|(name, _)| !name.as_str().starts_with("site.b"))
            .map(|(name, definition)| (name.clone(), definition.clone()))
            .collect();
        let (clock, hub) = (fixture.sim.clock(), fixture.hub.clone());
        fixture.tasks.spawn(async move {
            clock
                .sleep(Span::from_nanos(Span::SECOND.nanos() / 10))
                .await;
            hub.set_definitions(&kept);
        });
        let (head, _) = status(&node, "@admin").await;
        let connectors = [unknown("site.a", "steady"), unknown("site.b", "steady")];
        assert_eq!(head, json!({ "output": { "connectors": connectors } }));
    });
}

#[test]
fn waits_one_second_from_the_request_also_while_the_node_reads_a_new_spec() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let a = connector("site.a", "steady", "edge");
        applied(&fixture, &node, &a).await;
        let extra = (0..500).map(|n| {
            format!("channel \"site.a.extra{n}.time\" {{ kind = \"index\" }}\n")
        });
        let text: Vec<_> = std::iter::once(a).chain(extra).collect();
        applied_alone(&node, &text.concat()).await;
        let (head, _, waited) = timed_status(&fixture, &node, "@admin").await;
        assert_eq!(waited, Span::SECOND);
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
    });
}

/// The text of a placement that homes each channel on the node `edge`.
const EDGE: &str = "placement \"p\" {\n  select = \"**\"\n  home = \"edge\"\n}\n";

#[test]
fn gives_unknown_while_the_hub_holds_old_status_names_on_another_index() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let old = "channel \"x.time\" { kind = \"index\" }\n\
                   channel \"site.a.status.time\" { kind = \"index\" }\n\
                   channel \"site.a.status.state\" {\n  data_type = \"u8\"\n  \
                   index = \"x.time\"\n}\n";
        applied(&fixture, &node, &[old, EDGE].concat()).await;
        applied_alone(&node, &connector("site.a", "steady", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

/// The name of the `state` channel of `site.a`.
const STATE: &str = "site.a.status.state";

/// The status channels of `site.a` of `steady` but the index and [`STATE`], with the
/// sample type of each.
fn others() -> Vec<(Name, Type)> {
    let counts = [name("in"), name("confirmed")];
    let channels = connector::status::channels(&name("site.a"), &counts);
    let mut channels = channels.expect("short status names").1;
    channels.retain(|(channel, _)| channel.as_str() != STATE);
    channels
}

/// The text of user channels with the status names of `site.a` of `steady`, each data
/// channel on the index `index`, and [`STATE`] of the type `state`.
fn user_status(index: &str, state: &str) -> String {
    let on = |(channel, data_type): (&str, String)| {
        format!(
            "channel \"{channel}\" {{\n  data_type = \"{data_type}\"\n  \
             index = \"{index}\"\n}}\n"
        )
    };
    let index = format!("channel \"{index}\" {{ kind = \"index\" }}\n");
    let others = others();
    let others = others
        .iter()
        .map(|(channel, sample)| (channel.as_str(), sample.to_string()));
    let channels = std::iter::once((STATE, state.to_owned()))
        .chain(others)
        .map(on);
    std::iter::once(index)
        .chain(channels)
        .chain([EDGE.to_owned()])
        .collect()
}

/// Writes one frame of `index` and each data channel of [`user_status`], with `state`
/// as the sample of [`STATE`] and zeros for each other.
async fn write_user(fixture: &Fixture, index: &str, state: &[u8]) {
    let others = others();
    // Four zeros are one empty text.
    let zeros: Vec<_> = others
        .iter()
        .map(|(_, sample)| vec![0; sample.width().unwrap_or(4)])
        .collect();
    let others = others.iter().zip(&zeros);
    let others = others.map(|((channel, _), bytes)| (channel.as_str(), &bytes[..]));
    let samples: Vec<_> = std::iter::once((STATE, state)).chain(others).collect();
    write(fixture, index, &samples).await;
}

#[test]
fn gives_unknown_while_the_hub_holds_an_old_status_name_of_another_type() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let old = user_status("site.a.status.time", "uuid");
        applied(&fixture, &node, &old).await;
        write_user(&fixture, "site.a.status.time", &[7; 16]).await;
        applied_alone(&node, &connector("site.a", "steady", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_unknown_while_the_hub_holds_old_status_names_with_other_keys() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &connector("site.a", "steady", "edge")).await;
        supervise(&fixture, &node, &["site.a"]).await;
        applied_alone(
            &node,
            &["channel \"x.time\" { kind = \"index\" }\n", EDGE].concat(),
        )
        .await;
        applied_alone(&node, &connector("site.a", "steady", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn gives_unknown_while_the_hub_holds_old_status_names_on_an_index_of_no_status() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        applied(&fixture, &node, &user_status("x.time", "u8")).await;
        write_user(&fixture, "x.time", &[0]).await;
        applied_alone(&node, &connector("site.a", "steady", "edge")).await;
        let (head, rest) = status(&node, "@admin").await;
        let expected =
            json!({ "output": { "connectors": [unknown("site.a", "steady")] } });
        assert_eq!(head, expected);
        assert_eq!(rest, b"");
    });
}

#[test]
fn fails_when_the_buffer_fails_before_a_frame_of_the_status() {
    solo(|fixture, mesh| async move {
        let node = create_node(&fixture, mesh);
        let text = [
            connector("site.a", "steady", "edge"),
            connector("site.b", "steady", "edge"),
        ];
        applied(&fixture, &node, &text.concat()).await;
        fixture.sim.fail_file(
            std::path::Path::new("shard-0/ring"),
            env::files::Operation::Sync,
        );
        let samples: [(&str, &[u8]); 1] = [("site.b.status.in", &5_u64.to_le_bytes())];
        write(&fixture, "site.b.status.time", &samples).await;
        let (head, _) = status(&node, "@admin").await;
        let expected = json!({
            "errors": [{
                "code": "ops.channel-read",
                "message": "the status channels of the connector `site.a` did not \
                            read: the buffer of the shard failed: sync of \
                            shard-0/ring failed with OS error 5",
                "fix": "Fix the cause in the message, then ask for the status again",
                "notes": [],
            }],
            "status": 1,
        });
        assert_eq!(head, expected);
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
fn escapes_a_newline_in_each_value_of_the_table() {
    let error = Failure {
        class: Class::Device,
        text: "t".to_owned(),
    };
    let output = Output {
        connectors: vec![Status {
            name: "a\nb".to_owned(),
            kind: "k\nk".to_owned(),
            address: Some("x\ny".to_owned()),
            ..line("plant.plc", State::Running, None, Some(error))
        }],
    };
    let text = "CONNECTOR  KIND  STATE    ADDRESS  IN  CONFIRMED\n\
                a\\nb       k\\nk  running  x\\ny     -   -\n\
                \n\
                a\\nb: device error: t\n";
    assert_eq!(output.text(), text);
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
