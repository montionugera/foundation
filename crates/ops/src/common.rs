//! Fixtures that the tests of `plan`, `apply`, `status`, and `Node` share.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use connector::cancel;
use connector::kind::{self, Channels, Context, Table};
use document::Document;
use document::diagnostic::Diagnostic;
use env::files::{self, Operation};
use env::paths::Paths;
use env::tasks::Tasks;
use hub::Hub;
use mesh::Mesh;
use sim::Sim;
use spec::definition::Definition;
use types::channel::Key;
use types::name::Name;

use crate::front_end::{File, FrontEnd, FrontEnds};
use crate::{Failure, Node, Run};

#[path = "../tests/common/founding.rs"]
mod founding;

#[path = "../tests/common/quickstart.rs"]
mod quickstart;

pub(crate) use founding::{ADMIN, NODE, create_mesh};
pub(crate) use quickstart::{plant, shown};

pub(crate) const SITE: &str =
    include_str!("../../acceptance/tests/it/fixtures/site.hcl");

/// Lines that `ssh-keygen -t ed25519` wrote.
pub(crate) const BOB: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIP0QMDFGOHfS9XR71aVyCvs+QnNQ4BXrHs9dGDDz7KY6",
    " bob@site",
);
pub(crate) const EVE: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIGVVuOR8JKYpAcWLMUveadmJ1wUAmYGgIDtqlhFe7Yhg",
    " eve@laptop",
);
pub(crate) const SAM: &str = concat!(
    "ssh-ed25519 ",
    "AAAAC3NzaC1lZDI1NTE5AAAAIBwW/hh0ZkgH9jcODHef/ulXUH2Hx66uLYTxi9hWfS+o",
    " sam@site",
);

/// A kind whose channels are the labels of its `read` blocks, which it writes. It takes
/// each attribute, so it stands in for each kind of the fixtures.
pub(crate) struct Reader {
    /// The counts of the kind.
    counts: &'static [&'static str],
}

/// The stand-in of the `opcua` kind, with the counts that #435 gives it.
pub(crate) const OPCUA: Reader = Reader {
    counts: &["in", "lost", "missed"],
};

/// The stand-in of the `influx` kind, with the count `confirmed` that #1734 gives it.
pub(crate) const INFLUX: Reader = Reader {
    counts: &["confirmed"],
};

impl kind::Kind for Reader {
    type Config = Vec<Name>;

    fn parse(&self, config: &Document) -> Result<Vec<Name>, Vec<Diagnostic>> {
        let reads = config
            .blocks
            .iter()
            .filter(|block| &*block.keyword == "read");
        Ok(reads
            .map(|block| block.labels[0].text.parse().expect("a name"))
            .collect())
    }

    fn check(&self, writes: &Vec<Name>) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels {
            reader: None,
            writes: writes.clone(),
            counts: self.counts.iter().map(|count| name(count)).collect(),
        })
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, kind::Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    fn run(
        &self,
        _: Context<Vec<Name>>,
    ) -> impl Future<Output = Result<(), kind::Error>> {
        std::future::ready(Ok(()))
    }
}

pub(crate) fn front_ends() -> FrontEnds {
    FrontEnds::new(
        "hcl",
        FrontEnd {
            read: config_hcl::read,
        },
    )
}

/// The kinds of the connectors of the fixtures.
pub(crate) fn create_kinds() -> Table {
    Table::new().with("influx", INFLUX).with("opcua", OPCUA)
}

/// The sim node of a run, and a hub on it.
pub(crate) struct Fixture {
    pub(crate) sim: sim::node::Node,
    pub(crate) hub: Hub,
    /// The name of the member of the mesh on `sim`.
    pub(crate) name: Name,
    /// The tasks of the run.
    pub(crate) tasks: Tasks,
}

/// The node of `mesh` on `fixture`, whose channel keys count from 1.
pub(crate) fn create_node(fixture: &Fixture, mesh: Mesh) -> Node {
    create_node_reading(fixture, mesh, front_ends())
}

/// The node of `mesh` on `fixture` that reads with `front_ends`, whose channel keys
/// count from 1.
pub(crate) fn create_node_reading(
    fixture: &Fixture,
    mesh: Mesh,
    front_ends: FrontEnds,
) -> Node {
    let made = Cell::new(0);
    let key = move || {
        made.set(made.get() + 1);
        Key::from_u128(made.get())
    };
    Node::new(
        mesh,
        key,
        front_ends,
        Arc::new(create_kinds()),
        fixture.hub.clone(),
        fixture.sim.clock(),
        fixture.name.clone(),
    )
}

pub(crate) fn name(text: &str) -> Name {
    text.parse().expect("a name")
}

pub(crate) fn files(files: &[(&str, &str)]) -> Vec<File> {
    files
        .iter()
        .map(|(path, text)| File {
            path: PathBuf::from(path),
            text: (*text).to_owned(),
        })
        .collect()
}

/// The run, output, and errors of `foundation <args>`, whose requests go through
/// `reach`.
pub(crate) async fn cli<C>(
    paths: &Paths,
    args: &[&str],
    reach: impl AsyncFnMut(&Path) -> Result<C, Failure>,
) -> (Run, String, String)
where
    C: AsyncFn(&[u8]) -> Result<Vec<u8>, Failure>,
{
    let args = ["foundation"].iter().chain(args).map(Into::into);
    let (mut output, mut errors) = (Vec::new(), Vec::new());
    let input = std::io::empty();
    let run = crate::cli(args, input, &mut output, &mut errors, paths, reach).await;
    let text = |bytes| String::from_utf8(bytes).expect("UTF-8");
    (run, text(output), text(errors))
}

/// `site.hcl` with a placement that homes its index on `edge`.
pub(crate) fn placed_site() -> String {
    format!("{SITE}placement \"p\" {{\n  select = \"site.*\"\n  home = \"edge\"\n}}\n")
}

/// Runs `body` with the one node of a run and the mesh of [`create_mesh`] on it, whose
/// one member is `edge`, with the definitions of a first start.
pub(crate) fn solo<F: Future<Output = ()> + 'static>(
    body: impl FnOnce(Fixture, Mesh) -> F + Send + 'static,
) {
    founded(&["edge"], spec::founding::create(ADMIN.public()), body);
}

/// Runs `body` with the one node of a run and the mesh of [`create_mesh`] on it, with
/// the members `names` and the founding `definitions`.
pub(crate) fn founded<F: Future<Output = ()> + 'static>(
    names: &'static [&'static str],
    definitions: BTreeMap<Name, Definition>,
    body: impl FnOnce(Fixture, Mesh) -> F + Send + 'static,
) {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let ran = sim.run_on(&node, move |node, tasks| async move {
        let mesh = create_mesh(&node, &tasks, names, definitions).await;
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let (hub, _) = hub::testing::open(env).await;
        let fixture = Fixture {
            sim: node,
            hub,
            name: name(names[0]),
            tasks,
        };
        body(fixture, mesh).await;
    });
    assert_eq!(ran, Ok(()));
}

/// Makes each sync of the log of the mesh of [`create_mesh`] on `node` fail, and gives
/// the stop of its group at its next write of the log.
pub(crate) fn fail_sync(node: &sim::node::Node) -> mesh::Stopped {
    let path = Path::new("log").join("log-0");
    node.fail_file(&path, Operation::Sync);
    let cause = files::Error::Io {
        path,
        operation: Operation::Sync,
        code: 5,
    };
    mesh::Stopped::Write(mesh::log::Error::Files(cause))
}

/// Gives `Key::from_u128(n)` for each `n` from `from` up.
pub(crate) fn keys(from: u128) -> impl FnMut() -> Key {
    let mut next = from;
    move || {
        next += 1;
        Key::from_u128(next)
    }
}
