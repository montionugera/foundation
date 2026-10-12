//! The tests that bound the heap of `serve` with one count of the bytes it holds. The
//! count covers each thread, so this binary has no test harness. The sim runs on one
//! thread, so the count is exact.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use env::tasks::Tasks;
use mesh::card::addresses::Addresses;
use mesh::card::{self, Card};
use mesh::region::Founding;
use mesh::status::Status;
use mesh::{Member, Mesh};
use ops::{FrontEnd, FrontEnds, Node, serve};
use serde_json::{Value, json};
use sim::Sim;
use transport::{Port, Transport};
use types::channel::Key;
use types::ed25519::PrivateKey;
use types::name::Prefix;
use types::node::{self, SealKey};
use types::time::Span;
use wire::hub::client::BODY_BYTES_MAX;

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

const NODE: node::Key = node::Key::from_u128(1);
const PRIVATE_KEY: PrivateKey = PrivateKey([1; 32]);
const ADMIN: PrivateKey = PrivateKey([7; 32]);
/// The bytes of the path of the file of each request. Each problem or change of a reply
/// holds it, and the heap holds it once for each request.
const PATH_BYTES: usize = 64 << 10;
/// The problems or changes of each reply: with the path, 8 times the bound.
const ENTRIES: usize = 2_000;
/// The bytes of the request and of its steps, apart from the reply, that the heap can
/// hold at once. The steps took at most 3.9 MB on the laptop (Apple M3 Max).
const STEPS_BYTES: usize = 8 << 20;

fn main() {
    a_reply_of_many_problems_holds_no_more_than_the_bound();
    a_reply_of_many_changes_holds_no_more_than_the_bound();
}

fn a_reply_of_many_problems_holds_no_more_than_the_bound() {
    let text = "nope \"a\" {}\n".repeat(ENTRIES);
    serve_large(&text);
}

fn a_reply_of_many_changes_holds_no_more_than_the_bound() {
    let placement = "placement \"p\" {\n  select = \"site.*\"\n  home = \"edge\"\n}\n";
    let channels =
        (0..ENTRIES).map(|n| format!("channel \"site.c{n}\" {{ kind = \"index\" }}\n"));
    let text = std::iter::once(placement.to_owned())
        .chain(channels)
        .collect::<String>();
    serve_large(&text);
}

/// Serves a `plan` request of `text` at a long path, whose reply passes the bound, and
/// asserts that the heap grows by at most the bound and `STEPS_BYTES`.
fn serve_large(text: &str) {
    let path = format!("{}.hcl", "x".repeat(PATH_BYTES - 4));
    let body = format!(
        "{}\n",
        json!({ "plan": { "files": [{ "path": path, "text": text }] } })
    )
    .into_bytes();
    let mut sim = Sim::new(sim::Config::default());
    let sim_node = sim.node(sim::node::Config::default());
    let ran = sim.run_on(&sim_node, move |sim_node, tasks| async move {
        let mesh = open(&sim_node, &tasks).await;
        let front_ends = FrontEnds::new(
            "hcl",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        let node = Node::new(
            mesh,
            || Key::from_u128(1),
            front_ends,
            Arc::new(connector::kind::Table::new()),
        );
        let subject = "@admin".parse().expect("a name");
        let before = ALLOCATOR.held();
        ALLOCATOR.reset_peak();
        let reply = serve(&node, &subject, &body).await;
        let grown = ALLOCATOR.peak() - before;
        let bound = usize::try_from(BODY_BYTES_MAX).expect("the bound fits");
        assert!(
            grown <= bound + STEPS_BYTES,
            "a reply of at most {bound} bytes grew the heap by {grown}",
        );
        let end = reply
            .iter()
            .position(|byte| *byte == b'\n')
            .expect("a line");
        let head: Value = serde_json::from_slice(&reply[..end]).expect("JSON");
        assert_eq!(head["errors"][0]["code"], "ops.large-reply", "{head}");
    });
    assert_eq!(ran, Ok(()), "the run ends");
}

/// The mesh of a root region with the founding spec, whose one member and voter is
/// `edge` at `NODE`.
async fn open(node: &sim::node::Node, tasks: &Tasks) -> Mesh {
    let budget = block::Config::new(1 << 20).expect("the budget fits");
    let memory = block::Heap::new(budget.reservation());
    let pool = Rc::new(block::Pool::new(budget, memory));
    let at = SocketAddr::new(node.addresses()[0], 7000);
    let mut parts = Port::bind(&node.net(), at)
        .expect("a port")
        .split(NonZeroUsize::MIN);
    let config = transport::Config {
        private_key: PRIVATE_KEY,
        message_bytes_max: NonZeroUsize::new(1 << 16).expect("not zero"),
        window_bytes: 1 << 20,
        streams_max: NonZeroU32::new(16).expect("not zero"),
        idle: Span::from_nanos(60 * Span::SECOND.nanos()),
        clock: node.clock(),
        entropy: node.entropy(),
        tasks: tasks.clone(),
        pool: Rc::clone(&pool),
    };
    let part = parts.pop().expect("a part");
    let transport = Transport::new(config, part).expect("a transport");
    let store = blob::Store::open(blob::Config {
        files: node.files(),
        dir: "blob".into(),
        pool: Rc::clone(&pool),
        floor_bytes: 0,
    })
    .await
    .expect("a store");
    let card = Card {
        name: "edge".parse().expect("a name"),
        public_key: PRIVATE_KEY.public(),
        seal_key: SealKey::new([9; 32]).expect("a seal key"),
        addresses: Addresses::new(Vec::new()).expect("no addresses"),
        version: 1,
    };
    let member = Member {
        card: card::Signed::sign(NODE, card, &PRIVATE_KEY),
        admission: [0; 64],
        ephemeral: None,
        status: Status::new([].into()).expect("a status"),
    };
    let config = mesh::Config {
        key: NODE,
        private_key: PRIVATE_KEY,
        founding: Founding {
            prefix: Prefix::ROOT,
            voters: [NODE].into(),
            members: vec![member],
            definitions: spec::founding::create(ADMIN.public()),
            homes: BTreeMap::new(),
        },
        files: node.files(),
        dir: PathBuf::new(),
        clock: node.clock(),
        entropy: node.entropy(),
        tasks: tasks.clone(),
        transport: Rc::new(transport),
        pool,
        store: Rc::new(store),
    };
    Mesh::open(config).await.expect("a mesh")
}
