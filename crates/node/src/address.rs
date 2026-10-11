//! How a program on the node's host reaches the node, in the file `address` of the
//! data directory. The file holds the address while shard 0 serves the node's port,
//! and after a crash, and zero bytes after a clean stop.
//!
//! The file is 99 bytes: the tag, the node key (big-endian), the public key that the
//! node's transport proves, the family of the IP (4 or 6), the IP in 16 bytes (an
//! IPv4 address in the first 4), the port, the flow info, and the scope (each
//! big-endian), and the CRC32C of those 95 bytes (little-endian), in one sector
//! ([`crate::sector`]). For IPv4, the other 12 bytes of the IP, the flow info, and the
//! scope are zero.

use std::net::SocketAddr;
#[cfg(any(test, feature = "sim"))]
use std::net::{SocketAddrV4, SocketAddrV6};
use std::path::Path;
use std::pin::Pin;

use env::files::{File, Files};
use types::ed25519::PublicKey;

use crate::{Error, sector};

/// The name of the file.
pub(crate) const FILE: &str = "address";
/// The first bytes of the file; a new form gets a new tag.
const TAG: &[u8; 20] = b"foundation/address/1";
/// The length of the tag, the two keys, and the socket address.
const BODY: usize = 95;
/// The length of the file: the body and its CRC32C.
pub(crate) const LEN: usize = BODY + 4;

/// Where a node serves its port, with the keys that a program reaches it with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Address {
    /// The node's key.
    pub(crate) key: types::node::Key,
    /// The public key that the node's transport proves.
    pub(crate) public_key: PublicKey,
    /// Where the node's port is bound.
    pub(crate) bound: SocketAddr,
}

/// Where a program dials `port`: its UDP socket, as it is bound.
pub(crate) fn bound(port: &transport::Port) -> SocketAddr {
    let udp = port
        .addresses()
        .into_iter()
        .find_map(|address| match address {
            transport::Address::Udp(at) => Some(at),
            transport::Address::Tcp(_) | transport::Address::Relay { .. } => None,
        });
    udp.expect("invariant: a port binds a UDP socket")
}

/// Holds the file `address` of `files` with `address` while `serve` runs: writes it in
/// place, awaits `serve`, and writes it empty. The first hold on a data directory
/// makes the file, and a later one writes in its room, so it needs no free bytes.
/// Writes over a file of another length after it removes it. When the write fails,
/// does not poll `serve`, and writes the file empty. Does not drop `serve`: the caller
/// drops it after it has read the error. Call it under the lock of the data directory.
///
/// # Errors
///
/// [`Error::Directory`] for a file call that fails: one of the write, else the empty
/// write.
pub(crate) async fn hold(
    files: &Files,
    address: &Address,
    serve: Pin<&mut impl Future<Output = ()>>,
) -> Result<(), Error> {
    let file = open(files).await.map_err(Error::Directory)?;
    let written = sector::write(files, &file, &encode(address)).await;
    let emptied = if written.is_ok() {
        serve.await;
        sector::clear::<LEN>(&file).await
    } else {
        // The write can leave the address, and a failed sync poisons `file`.
        file.close().await;
        match files.open(Path::new(FILE), env::files::Mode::Write).await {
            Ok(file) => sector::clear::<LEN>(&file).await,
            Err(error) => Err(error),
        }
    };
    written.and(emptied).map_err(Error::Directory)
}

/// Opens the file to write, made of zero bytes when it is not there, and made again
/// when it has another length.
async fn open(files: &Files) -> Result<File, env::files::Error> {
    let path = Path::new(FILE);
    let opened = match sector::open::<LEN>(files, path, TAG).await {
        Err(env::files::Error::Length { .. }) => {
            files.remove(path).await?;
            sector::open(files, path, TAG).await
        }
        opened => opened,
    };
    Ok(opened?.0)
}

/// The bytes of the file that holds `address`.
fn encode(address: &Address) -> [u8; LEN] {
    let mut bytes = [0; LEN];
    bytes[..20].copy_from_slice(TAG);
    bytes[20..36].copy_from_slice(&address.key.as_u128().to_be_bytes());
    bytes[36..68].copy_from_slice(&address.public_key.to_bytes());
    match address.bound {
        SocketAddr::V4(at) => {
            bytes[68] = 4;
            bytes[69..73].copy_from_slice(&at.ip().octets());
        }
        SocketAddr::V6(at) => {
            bytes[68] = 6;
            bytes[69..85].copy_from_slice(&at.ip().octets());
            bytes[87..91].copy_from_slice(&at.flowinfo().to_be_bytes());
            bytes[91..BODY].copy_from_slice(&at.scope_id().to_be_bytes());
        }
    }
    bytes[85..87].copy_from_slice(&address.bound.port().to_be_bytes());
    sector::checksum(&mut bytes);
    bytes
}

/// The address in `bytes`, or `None` for bytes that [`encode`] does not write.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn decode(bytes: &[u8; LEN]) -> Option<Address> {
    fields(&sector::written(bytes, TAG)?)
}

/// The address in `bytes`, whatever its tag and checksum, or `None` for a public key
/// of small order, a family that is not 4 or 6, or an IPv4 address with a byte that
/// is not zero after its 4, in its flow info, or in its scope.
#[cfg(any(test, feature = "sim"))]
fn fields(bytes: &[u8; LEN]) -> Option<Address> {
    let word = |at: usize| {
        u32::from_be_bytes(bytes[at..at + 4].try_into().expect("invariant: 4 bytes"))
    };
    let port = u16::from_be_bytes([bytes[85], bytes[86]]);
    let bound = match bytes[68] {
        4 if bytes[73..85] == [0; 12] && bytes[87..BODY] == [0; 8] => {
            let ip: [u8; 4] = bytes[69..73].try_into().expect("invariant: 4 bytes");
            SocketAddr::V4(SocketAddrV4::new(ip.into(), port))
        }
        6 => {
            let ip: [u8; 16] = bytes[69..85].try_into().expect("invariant: 16 bytes");
            SocketAddr::V6(SocketAddrV6::new(ip.into(), port, word(87), word(91)))
        }
        _ => return None,
    };
    let public_key = bytes[36..68].try_into().expect("invariant: 32 bytes");
    Some(Address {
        key: types::node::Key::from_u128(u128::from_be_bytes(
            bytes[20..36].try_into().expect("invariant: 16 bytes"),
        )),
        public_key: PublicKey::new(public_key).ok()?,
        bound,
    })
}

/// Checks that `decode` gives an address exactly for the bytes that `encode` writes,
/// and that the address encodes to `bytes`.
///
/// # Panics
///
/// When a check fails.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn check(bytes: &[u8; LEN]) {
    let valid = fields(bytes).is_some_and(|address| encode(&address) == *bytes);
    match decode(bytes) {
        Some(address) => {
            assert!(valid, "decodes {bytes:02x?}");
            assert_eq!(encode(&address), *bytes, "encodes {bytes:02x?}");
        }
        None => assert!(!valid, "refuses {bytes:02x?}"),
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use proptest::prelude::*;
    use types::ed25519::PrivateKey;

    use super::*;

    /// Any address: a public key of any private key, and a socket address of each
    /// family with any flow info and scope.
    fn address() -> impl Strategy<Value = Address> {
        let v4 = any::<([u8; 4], u16)>()
            .prop_map(|(ip, port)| SocketAddr::V4(SocketAddrV4::new(ip.into(), port)));
        let v6 =
            any::<([u8; 16], u16, u32, u32)>().prop_map(|(ip, port, flow, scope)| {
                SocketAddr::V6(SocketAddrV6::new(ip.into(), port, flow, scope))
            });
        (any::<u128>(), any::<[u8; 32]>(), prop_oneof![v4, v6]).prop_map(
            |(key, private_key, bound)| Address {
                key: types::node::Key::from_u128(key),
                public_key: PrivateKey(private_key).public(),
                bound,
            },
        )
    }

    #[test]
    fn writes_the_tag_the_keys_an_ipv4_address_and_the_checksum() {
        let public_key = PrivateKey([7; 32]).public();
        let bytes = encode(&Address {
            key: types::node::Key::from_u128(0x0102),
            public_key,
            bound: SocketAddr::from((Ipv4Addr::LOCALHOST, 0x1f40)),
        });
        assert_eq!(&bytes[..20], b"foundation/address/1");
        assert_eq!(bytes[20..34], [0; 14]);
        assert_eq!(bytes[34..36], [1, 2], "big-endian");
        assert_eq!(bytes[36..68], public_key.to_bytes());
        assert_eq!(bytes[68..73], [4, 127, 0, 0, 1]);
        assert_eq!(bytes[73..85], [0; 12]);
        assert_eq!(bytes[85..87], [0x1f, 0x40], "big-endian");
        assert_eq!(bytes[87..95], [0; 8]);
        assert_eq!(
            bytes[95..],
            crc32c::crc32c(&bytes[..95]).to_le_bytes(),
            "little-endian"
        );
    }

    #[test]
    fn writes_an_ipv6_address_with_its_flow_info_and_scope() {
        let ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
        let bytes = encode(&Address {
            key: types::node::Key::from_u128(1),
            public_key: PrivateKey([7; 32]).public(),
            bound: SocketAddrV6::new(ip, 7000, 0x0102_0304, 0x0506_0708).into(),
        });
        assert_eq!(bytes[68], 6);
        assert_eq!(bytes[69..85], ip.octets());
        assert_eq!(bytes[85..87], 7000u16.to_be_bytes());
        assert_eq!(bytes[87..91], [1, 2, 3, 4], "big-endian");
        assert_eq!(bytes[91..95], [5, 6, 7, 8], "big-endian");
    }

    #[test]
    fn refuses_zero_bytes() {
        assert_eq!(decode(&[0; LEN]), None);
    }

    /// The input `address-valid-v6` of the fuzz corpus holds an IPv6 address with a
    /// flow info and a scope.
    #[test]
    fn decodes_the_ipv6_address_of_the_corpus() {
        let valid =
            include_bytes!("../../../oracles/fuzz/node_identity/address-valid-v6");
        let ip = Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1);
        let address = Address {
            key: types::node::Key::from_u128(2),
            public_key: PrivateKey([7; 32]).public(),
            bound: SocketAddr::V6(SocketAddrV6::new(ip, 7001, 0x0102_0304, 5)),
        };
        assert_eq!(decode(valid), Some(address));
    }

    /// The bytes of `address` with `change` made to them and the checksum of the
    /// result.
    fn changed(address: &Address, change: impl FnOnce(&mut [u8; LEN])) -> [u8; LEN] {
        let mut bytes = encode(address);
        change(&mut bytes);
        sector::checksum(&mut bytes);
        bytes
    }

    #[test]
    fn refuses_a_public_key_of_small_order() {
        let address = Address {
            key: types::node::Key::from_u128(1),
            public_key: PrivateKey([7; 32]).public(),
            bound: SocketAddr::from((Ipv4Addr::LOCALHOST, 7000)),
        };
        assert_eq!(decode(&changed(&address, |_| ())), Some(address));
        // The point of order 1: y = 1.
        let small = changed(&address, |bytes| {
            bytes[36..68].copy_from_slice(&[0; 32]);
            bytes[36] = 1;
        });
        assert_eq!(decode(&small), None);
        check(&small);
    }

    proptest! {
        #[test]
        fn decodes_what_it_encodes(address in address()) {
            prop_assert_eq!(decode(&encode(&address)), Some(address));
        }

        #[test]
        fn refuses_each_one_bit_change(address in address(), bit in 0..LEN * 8) {
            let mut bytes = encode(&address);
            bytes[bit / 8] ^= 1 << (bit % 8);
            prop_assert_eq!(decode(&bytes), None);
        }

        #[test]
        fn refuses_each_one_bit_change_of_the_tag_with_its_checksum(
            address in address(),
            bit in 0..TAG.len() * 8,
        ) {
            check(&encode(&address));
            let bytes = changed(&address, |bytes| bytes[bit / 8] ^= 1 << (bit % 8));
            prop_assert_eq!(decode(&bytes), None);
        }

        #[test]
        fn refuses_each_other_family_with_its_checksum(
            address in address(),
            family in any::<u8>().prop_filter("not a family", |&f| f != 4 && f != 6),
        ) {
            let bytes = changed(&address, |bytes| bytes[68] = family);
            prop_assert_eq!(decode(&bytes), None);
        }

        /// An IPv4 address has one form: each bit after its 4 bytes, and of the flow
        /// info and the scope, is zero.
        #[test]
        fn refuses_an_ipv4_address_with_a_bit_that_ipv6_holds(
            address in address(),
            ip in any::<[u8; 4]>(),
            bit in (0..20 * 8usize),
        ) {
            let bound = SocketAddr::from((ip, address.bound.port()));
            let address = Address { bound, ..address };
            // The 12 bytes after the IP, then the 8 of the flow info and the scope.
            let at = if bit < 96 { 73 * 8 + bit } else { 87 * 8 + bit - 96 };
            let bytes = changed(&address, |bytes| bytes[at / 8] ^= 1 << (at % 8));
            prop_assert_eq!(decode(&bytes), None);
        }

        #[test]
        fn checks_any_bytes(bytes in any::<[u8; LEN]>()) {
            check(&bytes);
        }

        #[test]
        fn checks_any_body_with_its_checksum(mut bytes in any::<[u8; LEN]>()) {
            sector::checksum(&mut bytes);
            check(&bytes);
        }

        /// Any keys and socket bytes after the tag and a family, so `check` meets
        /// bytes that decode.
        #[test]
        fn checks_any_body_with_the_tag_a_family_and_its_checksum(
            mut bytes in any::<[u8; LEN]>(),
            v6 in any::<bool>(),
        ) {
            bytes[..20].copy_from_slice(TAG);
            bytes[68] = if v6 { 6 } else { 4 };
            sector::checksum(&mut bytes);
            check(&bytes);
        }
    }
}
