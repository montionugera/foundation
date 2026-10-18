//! A node process whose spec holds the plant channels, and the node and admin of a
//! node process. The tests of `sdk` and the `sdk` bench use them.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::rc::Rc;

use env::tasks::Tasks;
use sdk::write::{Config, Writer};
use sdk::{Client, Node, Subject};
use types::ed25519::PrivateKey;

use crate::rig::Rig;

/// The bytes of the file `name` of the data directory of `rig`, once it holds `tag`.
/// Checks the CRC32C of the file.
fn read<const LEN: usize>(rig: &Rig, name: &str, tag: &[u8]) -> [u8; LEN] {
    let path = rig.dir.join("foundation-data/data").join(name);
    let bytes = rig.wait(&format!("the node writes {name}"), || held(&path, tag));
    let body = LEN - 4;
    assert_eq!(
        crc32c::crc32c(&bytes[..body]).to_le_bytes(),
        bytes[body..],
        "the CRC32C of {name}"
    );
    bytes
}

/// The bytes at `path` when they are `LEN` long and start with `tag`.
#[expect(
    clippy::disallowed_methods,
    reason = "the test reads a file of a node process"
)]
fn held<const LEN: usize>(path: &Path, tag: &[u8]) -> Result<[u8; LEN], String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match <[u8; LEN]>::try_from(bytes) {
        Ok(bytes) if bytes.starts_with(tag) => Ok(bytes),
        Ok(bytes) => Err(format!("{bytes:?}")),
        Err(bytes) => Err(format!("{} bytes", bytes.len())),
    }
}

/// The node of `rig`, with an unspecified IP as loopback, and its admin, from the
/// files `address` and `admin.key`.
pub(crate) fn admin(rig: &Rig) -> (Node, Subject) {
    let address: [u8; 99] = read(rig, "address", b"foundation/address/1");
    let key: [u8; 54] = read(rig, "admin.key", b"foundation/admin/1");
    let port = u16::from_be_bytes([address[85], address[86]]);
    let ip = match address[68] {
        4 => IpAddr::from(<[u8; 4]>::try_from(&address[69..73]).unwrap()),
        6 => IpAddr::from(<[u8; 16]>::try_from(&address[69..85]).unwrap()),
        family => panic!("the address has family {family}"),
    };
    let ip = match ip {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    let node = u128::from_be_bytes(address[20..36].try_into().unwrap());
    let public_key = <[u8; 32]>::try_from(&address[36..68]).unwrap();
    let node = Node {
        addresses: vec![SocketAddr::new(ip, port)],
        key: types::node::Key::from_u128(node),
        public_key: types::ed25519::PublicKey::new(public_key).unwrap(),
    };
    let subject = Subject {
        name: spec::founding::admin(),
        private_key: PrivateKey(key[18..50].try_into().unwrap()),
    };
    (node, subject)
}

/// The channels of the tests of writers: the index `plant.time`, and `plant.a` and
/// `plant.b` of `i64` on it, homed at the node `edge`.
const CHANNELS: &str = concat!(
    "channel \"plant.time\" { kind = \"index\" }\n",
    "channel \"plant.a\" {\n  data_type = \"i64\"\n  index = \"plant.time\"\n}\n",
    "channel \"plant.b\" {\n  data_type = \"i64\"\n  index = \"plant.time\"\n}\n",
    "placement \"plant.home\" {\n  select = \"plant.*\"\n  home = \"edge\"\n}\n",
);

/// A rig whose node runs with [`CHANNELS`] applied, its node, and its admin. Waits
/// until the hub of the node knows the channels, which it learns after the apply.
pub(crate) fn planted() -> (Rig, Node, Subject) {
    let mut rig = Rig::new();
    rig.config(CHANNELS);
    rig.start();
    rig.apply();
    let (node, subject) = admin(&rig);
    let client = Client::connect(&node, subject.clone()).unwrap();
    rig.wait("the hub knows the channels", || {
        let channels = vec!["plant.a".parse().unwrap(), "plant.b".parse().unwrap()];
        let writer = client.open_writer(Config::new(channels));
        writer.and_then(Writer::close).map_err(|e| e.to_string())
    });
    assert_eq!(client.close(), Ok(()));
    (rig, node, subject)
}

/// A session with `node` as `subject`, as the client of `sdk` makes it.
pub(crate) async fn session(
    node: Node,
    subject: Subject,
    tasks: Tasks,
) -> program::Session {
    let budget = |bytes| block::Config::new(bytes).unwrap();
    let clock = os::clock();
    let entropy = os::entropy();
    let pool = Rc::new(block::Pool::heap(budget(1 << 21)));
    let transport = transport::Client::new(transport::client::Config {
        net: os::net(),
        clock: clock.clone(),
        entropy: entropy.clone(),
        tasks: tasks.clone(),
        pool: Rc::clone(&pool),
    })
    .unwrap();
    let config = program::Config {
        via: node.key,
        node: node.public_key,
        addresses: node
            .addresses
            .into_iter()
            .map(transport::Address::Udp)
            .collect(),
        subject: subject.name,
        key: subject.private_key,
        clock,
        entropy,
        tasks,
        pool,
        frames: Rc::new(block::Pool::heap(budget(64 << 10))),
    };
    program::Session::connect(&transport, config).await.unwrap()
}
