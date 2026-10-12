//! `ops::serve` never panics on a request body, and each reply has the form of an
//! output or of an error, within the bound of a reply.
//!
//! Input: the body of one request. The subject is the admin of a new mesh with one
//! member, so a request of the right form passes the access check and runs. The reply
//! of a `plan` holds a plan file that decodes, and an `apply` of that file then gives
//! an output.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use connector::kind::Table;
use env::tasks::Tasks;
use libfuzzer_sys::fuzz_target;
use mesh::card::addresses::Addresses;
use mesh::card::{self, Card};
use mesh::region::Founding;
use mesh::status::Status;
use mesh::{Member, Mesh};
use ops::{FrontEnd, FrontEnds, Node};
use transport::{Port, Transport};
use types::channel;
use types::ed25519::PrivateKey;
use types::name::{Name, Prefix};
use types::node::{self, SealKey};
use types::time::Span;
use wire::hub::client::BODY_BYTES_MAX;

const KEY: node::Key = node::Key::from_u128(1);
const PRIVATE_KEY: PrivateKey = PrivateKey([1; 32]);
const ADMIN: PrivateKey = PrivateKey([7; 32]);
const OUTPUT: &[u8] = b"{\"output\":{";
const ERRORS: &[u8] = b"{\"errors\":[{\"code\":\"";

fn name(text: &str) -> Name {
    text.parse().expect("a name")
}

/// The mesh of a root region whose one member, `edge`, is this node, with the
/// definitions of a first start.
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
        name: name("edge"),
        public_key: PRIVATE_KEY.public(),
        seal_key: SealKey::new([9; 32]).expect("a seal key"),
        addresses: Addresses::new(Vec::new()).expect("no addresses"),
        version: 1,
    };
    let member = Member {
        card: card::Signed::sign(KEY, card, &PRIVATE_KEY),
        admission: [0; 64],
        ephemeral: None,
        status: Status::new([].into()).expect("a status"),
    };
    let config = mesh::Config {
        key: KEY,
        private_key: PRIVATE_KEY,
        founding: Founding {
            prefix: Prefix::ROOT,
            voters: [KEY].into(),
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

/// The first line of `reply` and the bytes after its newline, once the reply has the
/// form of an output or of an error within the bound.
fn checked(reply: &[u8]) -> (&[u8], &[u8]) {
    assert!(u64::try_from(reply.len()).expect("a count") <= BODY_BYTES_MAX);
    let end = reply.iter().position(|&byte| byte == b'\n');
    let end = end.expect("a first line");
    let (head, rest) = (&reply[..end], &reply[end + 1..]);
    if head.starts_with(ERRORS) {
        let ends = [b",\"status\":1}", b",\"status\":2}"];
        assert!(ends.iter().any(|end| head.ends_with(*end)), "{head:?}");
        assert_eq!(rest, b"");
    } else {
        assert!(head.starts_with(OUTPUT), "{head:?}");
    }
    (head, rest)
}

fuzz_target!(|input: &[u8]| {
    let body = input.to_vec();
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    let ran = sim.run_on(&host, move |host, tasks| async move {
        let made = Cell::new(0);
        let key = move || {
            made.set(made.get() + 1);
            channel::Key::from_u128(made.get())
        };
        let front_ends = FrontEnds::new(
            "hcl",
            FrontEnd {
                read: config_hcl::read,
            },
        );
        let kinds = Table::new().with("influx", connector_influx::Kind::default());
        let mesh = open(&host, &tasks).await;
        let node = Node::new(mesh, key, front_ends, Arc::new(kinds));
        let admin = name("@admin");
        let reply = ops::serve(&node, &admin, &body).await;
        let (head, plan) = checked(&reply);
        if !head.starts_with(b"{\"output\":{\"base\":") {
            assert!(head.starts_with(ERRORS) || plan.is_empty(), "{head:?}");
            return;
        }
        config::plan::Plan::decode(plan).expect("a plan file");
        let mut apply = b"{\"apply\":{}}\n".to_vec();
        apply.extend(plan);
        let reply = ops::serve(&node, &admin, &apply).await;
        let (head, rest) = checked(&reply);
        assert!(head.starts_with(b"{\"output\":{\"pointer\":"), "{head:?}");
        assert_eq!(rest, b"");
    });
    assert_eq!(ran, Ok(()));
});
