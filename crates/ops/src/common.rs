//! Fixtures that the tests of `plan`, `apply`, and `Node` share.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use connector::cancel;
use connector::kind::{self, Channels, Context, Table};
use document::Document;
use document::diagnostic::Diagnostic;
use env::files::{self, Operation};
use mesh::Mesh;
use sim::Sim;
use spec::definition::Definition;
use types::channel::Key;
use types::name::Name;

use crate::Node;
use crate::front_end::{File, FrontEnd, FrontEnds};

#[path = "../tests/common/founding.rs"]
mod founding;

pub(crate) use founding::{ADMIN, NODE, create_mesh};

pub(crate) const SITE: &str =
    include_str!("../../acceptance/tests/it/fixtures/site.hcl");

const PAGE: &str = include_str!("../../../docs/quickstart.md");

/// The text of each fenced block of the quickstart page whose language is `language`,
/// as `blocks` of `crates/node/tests/it/quickstart.rs` gives it.
fn blocks(language: &str) -> impl Iterator<Item = &'static str> + '_ {
    PAGE.split("```")
        .skip(1)
        .step_by(2)
        .filter_map(move |block| block.strip_prefix(language)?.strip_prefix('\n'))
}

/// The config of the quickstart page, its one `hcl` block.
pub(crate) fn plant() -> &'static str {
    let [hcl] = blocks("hcl").collect::<Vec<_>>()[..] else {
        panic!("the page has not one `hcl` block");
    };
    hcl
}

/// The output that the quickstart page shows, its one `text` block that starts with
/// `start`.
pub(crate) fn shown(start: &str) -> &'static str {
    let shown = blocks("text").filter(|block| block.starts_with(start));
    let [text] = shown.collect::<Vec<_>>()[..] else {
        panic!("the page has not one `text` block that starts with {start:?}");
    };
    text
}

/// A kind whose channels are the labels of its `read` blocks, which it writes. It takes
/// each attribute, so it stands in for each kind of the fixtures.
pub(crate) struct Reader;

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
            counts: Vec::new(),
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
    Table::new().with("influx", Reader).with("opcua", Reader)
}

/// The node of `mesh`, whose channel keys count from 1.
pub(crate) fn create_node(mesh: Mesh) -> Node {
    create_node_reading(mesh, front_ends())
}

/// The node of `mesh` that reads with `front_ends`, whose channel keys count from 1.
pub(crate) fn create_node_reading(mesh: Mesh, front_ends: FrontEnds) -> Node {
    let made = Cell::new(0);
    let key = move || {
        made.set(made.get() + 1);
        Key::from_u128(made.get())
    };
    Node::new(mesh, key, front_ends, Arc::new(create_kinds()))
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

/// `site.hcl` with a placement that homes its index on `edge`.
pub(crate) fn placed_site() -> String {
    format!("{SITE}placement \"p\" {{\n  select = \"site.*\"\n  home = \"edge\"\n}}\n")
}

/// Runs `body` with the one node of a run and the mesh of [`create_mesh`] on it, whose
/// one member is `edge`, with the definitions of a first start.
pub(crate) fn solo<F: Future<Output = ()> + 'static>(
    body: impl FnOnce(sim::node::Node, Mesh) -> F + Send + 'static,
) {
    founded(&["edge"], spec::founding::create(ADMIN.public()), body);
}

/// Runs `body` with the one node of a run and the mesh of [`create_mesh`] on it, with
/// the members `names` and the founding `definitions`.
pub(crate) fn founded<F: Future<Output = ()> + 'static>(
    names: &'static [&'static str],
    definitions: BTreeMap<Name, Definition>,
    body: impl FnOnce(sim::node::Node, Mesh) -> F + Send + 'static,
) {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let ran = sim.run_on(&node, move |node, tasks| async move {
        let mesh = create_mesh(&node, &tasks, names, definitions).await;
        body(node, mesh).await;
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
