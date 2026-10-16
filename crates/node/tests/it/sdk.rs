//! The Rust SDK against a node process.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;

use sdk::{Client, Credentials, Error};
use types::ed25519::PrivateKey;
use wire::hub::client::Refusal;

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

/// Where the node of `rig` serves its port, with an unspecified IP as loopback, and
/// the credentials of its admin, from the files `address` and `admin.key`.
fn admin(rig: &Rig) -> (SocketAddr, Credentials) {
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
    let credentials = Credentials {
        node: types::node::Key::from_u128(node),
        public_key: types::ed25519::PublicKey::new(public_key).unwrap(),
        subject: spec::founding::admin(),
        private_key: PrivateKey(key[18..50].try_into().unwrap()),
    };
    (SocketAddr::new(ip, port), credentials)
}

#[test]
fn a_client_connects_to_a_node_and_connects_again_after_its_drop() {
    let mut rig = Rig::new();
    rig.start();
    let (address, credentials) = admin(&rig);
    let client = Client::connect(address, credentials.clone()).unwrap();
    drop(client);
    let client = Client::connect(address, credentials).unwrap();
    drop(client);
    rig.stop();
}

#[test]
fn a_client_with_a_wrong_public_key_gets_the_error_of_the_handshake() {
    let mut rig = Rig::new();
    rig.start();
    let (address, credentials) = admin(&rig);
    let peer = PrivateKey([7; 32]).public();
    let credentials = Credentials {
        public_key: peer,
        ..credentials
    };
    let error = Client::connect(address, credentials).unwrap_err();
    let unreachable = transport::Error::Unreachable {
        peer,
        attempts: vec![(
            transport::Address::Udp(address),
            transport::Error::Authentication { expected: peer },
        )],
    };
    assert_eq!(
        error,
        Error::Session(program::Error::Transport(unreachable))
    );
    rig.stop();
}

#[test]
fn a_client_of_a_subject_that_the_spec_does_not_list_is_refused() {
    let mut rig = Rig::new();
    rig.start();
    let (address, credentials) = admin(&rig);
    let credentials = Credentials {
        subject: "plant.operator".parse().unwrap(),
        ..credentials
    };
    let error = Client::connect(address, credentials).unwrap_err();
    assert_eq!(
        error,
        Error::Session(program::Error::Refused(Refusal::Refused))
    );
    assert_eq!(error.to_string(), Refusal::Refused.to_string());
    rig.stop();
}

#[test]
fn a_client_of_an_address_that_no_node_reads_is_unreachable() {
    #[expect(clippy::disallowed_methods, reason = "a socket that never reads")]
    let silent = std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();
    let address = silent.local_addr().unwrap();
    let peer = PrivateKey([7; 32]).public();
    let credentials = Credentials {
        node: types::node::Key::from_u128(1),
        public_key: peer,
        subject: spec::founding::admin(),
        private_key: PrivateKey([8; 32]),
    };
    let error = Client::connect(address, credentials).unwrap_err();
    let unreachable = transport::Error::Unreachable {
        peer,
        attempts: vec![(transport::Address::Udp(address), transport::Error::TimedOut)],
    };
    assert_eq!(
        error,
        Error::Session(program::Error::Transport(unreachable))
    );
}
