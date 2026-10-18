use std::net::{Ipv4Addr, Ipv6Addr};

use proptest::prelude::*;
use types::ed25519::PrivateKey;
use types::name;

use super::*;
use crate::hub::client::First;

fn write(length: u64) -> Write {
    Write {
        length,
        signature: [0x5e; 64],
    }
}

fn key(byte: u8) -> PublicKey {
    PrivateKey([byte; 32]).public()
}

fn open(lease: Option<Span>, names: &[&str]) -> Open {
    Open {
        authority: Authority(3),
        lease,
        channels: names.iter().map(|name| name.parse().unwrap()).collect(),
    }
}

fn away(addresses: Vec<Address>) -> Away {
    Away {
        node: node::Key::from_u128(0x0102),
        public_key: key(7),
        addresses,
    }
}

fn v4(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), port))
}

fn encode_write(write: Write) -> Vec<u8> {
    let mut out = vec![0xaa; Write::LEN];
    write.encode(&mut out);
    out
}

fn encode_open(open: &Open) -> Vec<u8> {
    let mut out = vec![0xaa; open.encoded_len()];
    open.encode(&mut out);
    out
}

fn encode_away(away: &Away) -> Vec<u8> {
    let mut out = vec![0xaa; away.encoded_len()];
    away.encode(&mut out);
    out
}

fn encode_reply(reply: Reply) -> Vec<u8> {
    let mut out = vec![0xaa; reply.encoded_len()];
    reply.encode(&mut out);
    out
}

fn not_utf8(bytes: &[u8]) -> text::Error<Box<name::Error>> {
    text::Error::Utf8(std::str::from_utf8(bytes).unwrap_err())
}

/// `kind`, then `len - 1` zeros.
fn zeros(kind: u8, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    bytes[0] = kind;
    bytes
}

/// The bytes of an away with no addresses, then `count` and `rest`.
fn away_with(count: u8, rest: &[u8]) -> Vec<u8> {
    let mut bytes = encode_away(&away(Vec::new()));
    *bytes.last_mut().unwrap() = count;
    bytes.extend(rest);
    bytes
}

fn taken(seq: u64, count: u32) -> Reply {
    Reply::Outcome(Outcome::Taken {
        range: frame::Range { seq, count },
    })
}

const REFUSALS: [(Refusal, u8); 7] = [
    (Refusal::Waiting, 1),
    (Refusal::Reserved, 2),
    (Refusal::Expired, 3),
    (Refusal::Order, 4),
    (Refusal::Codec, 5),
    (Refusal::Full, 6),
    (Refusal::Large, 7),
];

mod pins {
    use super::*;

    #[test]
    fn the_write() {
        let mut bytes = vec![7, 0x03, 0x02, 0, 0, 0, 0, 0, 0];
        bytes.extend([0x5e; 64]);
        assert_eq!(encode_write(write(0x0203)), bytes);
    }

    #[test]
    fn the_open() {
        let mut bytes = vec![3, 0, 8];
        bytes.extend(b"site_a.x");
        bytes.push(1);
        bytes.extend(b"y");
        let none = open(None, &["site_a.x", "y"]);
        assert_eq!(none.encoded_len(), 13);
        assert_eq!(encode_open(&none), bytes);
        let lease = open(Some(Span::from_nanos(-0x0102)), &["y"]);
        let mut bytes = vec![3, 1];
        bytes.extend((-0x0102_i64).to_le_bytes());
        bytes.extend([1, b'y']);
        assert_eq!(lease.encoded_len(), 12);
        assert_eq!(encode_open(&lease), bytes);
    }

    #[test]
    fn the_away() {
        let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 0x0a0b));
        let away = away(vec![
            Address::Udp(v4(0x0304)),
            Address::Tcp(v6),
            Address::Relay {
                node: key(9),
                at: v4(5),
            },
        ]);
        let mut bytes = vec![7, 0x02, 0x01];
        bytes.extend([0; 14]);
        bytes.extend(key(7).to_bytes());
        bytes.push(3);
        bytes.extend([0, 4, 10, 0, 0, 1, 0x04, 0x03]);
        bytes.extend([1, 6]);
        bytes.extend(Ipv6Addr::LOCALHOST.octets());
        bytes.extend([0x0b, 0x0a]);
        bytes.push(2);
        bytes.extend(key(9).to_bytes());
        bytes.extend([4, 10, 0, 0, 1, 5, 0]);
        assert_eq!(away.encoded_len(), bytes.len());
        assert_eq!(encode_away(&away), bytes);
    }

    #[test]
    fn the_replies() {
        let range = frame::Range {
            seq: 0x0102,
            count: 0x0304,
        };
        let mut bytes = vec![1, 0x02, 0x01, 0, 0, 0, 0, 0, 0, 0x04, 0x03, 0, 0];
        assert_eq!(encode_reply(taken(0x0102, 0x0304)), bytes);
        bytes[0] = 2;
        let lost = Reply::Outcome(Outcome::Lost { range });
        assert_eq!(encode_reply(lost), bytes);
        for (refusal, code) in REFUSALS {
            let refused = Reply::Outcome(Outcome::Refused(refusal));
            assert_eq!(encode_reply(refused), [3, code]);
        }
        let stored = Reply::Stored { through: 0x0506 };
        assert_eq!(encode_reply(stored), [4, 0x06, 0x05, 0, 0, 0, 0, 0, 0]);
    }
}

mod first {
    use super::*;

    #[test]
    fn decodes_a_write() {
        let bytes = encode_write(write(4));
        assert_eq!(First::decode(&bytes), Ok(First::Write(write(4))));
    }

    #[test]
    fn gives_the_errors_of_a_write() {
        assert_eq!(First::decode(&[7]), Err(Error::Length { len: 1 }));
        let mut over = encode_write(write(1));
        over[1..9].copy_from_slice(&(OPEN_BYTES_MAX + 1).to_le_bytes());
        assert_eq!(
            First::decode(&over),
            Err(Error::Oversize {
                length: OPEN_BYTES_MAX + 1,
                cap: OPEN_BYTES_MAX,
            })
        );
    }
}

mod write {
    use super::*;

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|&kind| kind != 7) {
            assert_eq!(
                Write::decode(&zeros(kind, Write::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Write::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for len in [1, 9, 72, 74] {
            assert_eq!(Write::decode(&zeros(7, len)), Err(Error::Length { len }));
        }
    }

    #[test]
    fn checks_the_length_before_the_cap() {
        let mut long = zeros(7, Write::LEN + 1);
        long[1..9].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Write::decode(&long), Err(Error::Length { len: 74 }));
    }

    #[test]
    fn takes_an_open_at_the_cap_and_refuses_one_over_it() {
        let at = encode_write(write(OPEN_BYTES_MAX));
        assert_eq!(Write::decode(&at), Ok(write(OPEN_BYTES_MAX)));
        let mut over = at;
        over[1..9].copy_from_slice(&(OPEN_BYTES_MAX + 1).to_le_bytes());
        assert_eq!(
            Write::decode(&over),
            Err(Error::Oversize {
                length: OPEN_BYTES_MAX + 1,
                cap: OPEN_BYTES_MAX,
            })
        );
    }

    #[test]
    fn gives_its_body() {
        assert_eq!(write(5).body().remain(), 5);
        assert_eq!(write(0).body().remain(), 0);
    }

    #[test]
    #[should_panic(expected = "an open of 65537 bytes is over the cap of 65536")]
    fn panics_on_an_encode_over_the_cap() {
        encode_write(write(OPEN_BYTES_MAX + 1));
    }

    #[test]
    #[should_panic(expected = "an open of 65537 bytes is over the cap of 65536")]
    fn panics_on_a_body_over_the_cap() {
        drop(write(OPEN_BYTES_MAX + 1).body());
    }
}

mod open {
    use super::*;

    #[test]
    fn refuses_an_empty_open() {
        assert_eq!(Open::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_a_lease_byte_that_is_neither_0_nor_1() {
        for byte in 2..=u8::MAX {
            assert_eq!(
                Open::decode(&[3, byte, 1, b'a']),
                Err(Error::Lease { byte })
            );
        }
    }

    #[test]
    fn refuses_an_open_that_ends_inside_a_field() {
        assert_eq!(Open::decode(&[3]), Err(Error::Length { len: 1 }));
        let mut lease = vec![3, 1];
        lease.extend([0; 7]);
        assert_eq!(Open::decode(&lease), Err(Error::Length { len: 9 }));
        assert_eq!(
            Open::decode(&[3, 0, 2, b'a']),
            Err(Error::Length { len: 4 })
        );
    }

    #[test]
    fn refuses_an_open_that_names_no_channel() {
        assert_eq!(Open::decode(&[3, 0]), Err(Error::Unnamed));
        let mut lease = vec![3, 1];
        lease.extend(5_i64.to_le_bytes());
        assert_eq!(Open::decode(&lease), Err(Error::Unnamed));
    }

    #[test]
    fn refuses_a_name_that_does_not_read() {
        assert_eq!(
            Open::decode(&[3, 0, 1, 0xff]),
            Err(Error::Channel(not_utf8(&[0xff])))
        );
        assert_eq!(
            Open::decode(&[3, 0, 1, b'a', 0]),
            Err(Error::Channel(text::Error::Parse(Box::new(
                name::Error::Empty
            ))))
        );
    }

    #[test]
    fn gives_the_first_defect_in_the_order_of_the_bytes() {
        assert_eq!(
            Open::decode(&[3, 0, 1, 0xff, 2, b'a']),
            Err(Error::Channel(not_utf8(&[0xff])))
        );
    }

    #[test]
    fn decodes_a_name_of_255_bytes() {
        let long = "a".repeat(255);
        let open = open(None, &[&long]);
        assert_eq!(Open::decode(&encode_open(&open)), Ok(open));
    }

    #[test]
    #[should_panic(expected = "the open of a write names no channel")]
    fn panics_on_an_encode_with_no_channel() {
        encode_open(&open(None, &[]));
    }
}

mod away {
    use super::*;

    #[test]
    fn refuses_each_other_kind() {
        let mut bytes = away_with(0, &[]);
        for kind in (0..=u8::MAX).filter(|&kind| kind != 7) {
            bytes[0] = kind;
            assert_eq!(Away::decode(&bytes), Err(Error::Kind { kind }));
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Away::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_a_key_of_small_order() {
        let mut bytes = away_with(1, &[9]);
        bytes[17..49].copy_from_slice(&[0; 32]);
        assert_eq!(Away::decode(&bytes), Err(Error::SmallOrder));
    }

    #[test]
    fn takes_addresses_at_the_cap_and_refuses_more() {
        let at = away(vec![Address::Udp(v4(1)); ADDRESSES_MAX]);
        assert_eq!(Away::decode(&encode_away(&at)), Ok(at));
        assert_eq!(
            Away::decode(&away_with(33, &[9])),
            Err(Error::Addresses {
                count: 33,
                cap: ADDRESSES_MAX
            })
        );
    }

    #[test]
    fn refuses_an_address_kind_that_no_address_has() {
        for kind in 3..=u8::MAX {
            assert_eq!(
                Away::decode(&away_with(1, &[kind, 4, 10, 0, 0, 1, 0, 0])),
                Err(Error::Address { kind })
            );
        }
    }

    #[test]
    fn refuses_a_relay_key_of_small_order() {
        let mut rest = vec![2];
        rest.extend([0; 32]);
        rest.extend([4, 10, 0, 0, 1, 0, 0]);
        assert_eq!(Away::decode(&away_with(1, &rest)), Err(Error::SmallOrder));
    }

    #[test]
    fn refuses_a_family_that_is_neither_4_nor_6() {
        for family in (0..=u8::MAX).filter(|family| ![4, 6].contains(family)) {
            assert_eq!(
                Away::decode(&away_with(1, &[1, family, 10, 0, 0, 1, 0, 0])),
                Err(Error::Family { family })
            );
        }
    }

    #[test]
    fn refuses_an_away_that_ends_inside_an_address_or_after_the_last() {
        let short = away_with(1, &[0, 4, 10, 0, 0, 1, 0]);
        assert_eq!(Away::decode(&short), Err(Error::Length { len: 57 }));
        let missing = away_with(2, &[0, 4, 10, 0, 0, 1, 0, 0]);
        assert_eq!(Away::decode(&missing), Err(Error::Length { len: 58 }));
        let long = away_with(1, &[0, 4, 10, 0, 0, 1, 0, 0, 9]);
        assert_eq!(Away::decode(&long), Err(Error::Length { len: 59 }));
        assert_eq!(Away::decode(&[7]), Err(Error::Length { len: 1 }));
    }

    #[test]
    #[should_panic(expected = "33 addresses are over the cap of 32")]
    fn panics_on_an_encode_over_the_cap() {
        encode_away(&away(vec![Address::Udp(v4(1)); 33]));
    }

    #[test]
    #[should_panic(expected = "has flow info or a scope")]
    fn panics_on_an_encode_of_a_scoped_address() {
        let scoped = SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1, 0, 2);
        encode_away(&away(vec![Address::Tcp(SocketAddr::V6(scoped))]));
    }
}

mod reply {
    use super::*;

    #[test]
    fn refuses_each_kind_that_no_reply_has() {
        for kind in (0..=u8::MAX).filter(|kind| !(1..=4).contains(kind)) {
            assert_eq!(Reply::decode(&zeros(kind, 13)), Err(Error::Kind { kind }));
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Reply::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for (kind, len) in [(1, 12), (1, 14), (2, 1), (2, 14), (3, 1), (3, 3), (4, 8)] {
            assert_eq!(
                Reply::decode(&zeros(kind, len)),
                Err(Error::Length { len }),
                "{kind}"
            );
        }
        assert_eq!(Reply::decode(&zeros(4, 10)), Err(Error::Length { len: 10 }));
    }

    #[test]
    fn refuses_a_code_that_no_refusal_has() {
        for code in (0..=u8::MAX).filter(|code| !(1..=7).contains(code)) {
            assert_eq!(Reply::decode(&[3, code]), Err(Error::Refused { code }));
        }
    }

    #[test]
    fn checks_the_length_before_the_code() {
        assert_eq!(Reply::decode(&[3, 0, 0]), Err(Error::Length { len: 3 }));
    }

    #[test]
    fn decodes_each_refusal() {
        for (refusal, code) in REFUSALS {
            assert_eq!(
                Reply::decode(&[3, code]),
                Ok(Reply::Outcome(Outcome::Refused(refusal)))
            );
        }
    }
}

fn name() -> impl Strategy<Value = Name> {
    "[a-z][a-z0-9_]{0,6}(\\.[a-z0-9_]{1,6}){0,2}".prop_map(|text| text.parse().unwrap())
}

fn socket() -> impl Strategy<Value = SocketAddr> {
    prop_oneof![
        (any::<[u8; 4]>(), any::<u16>())
            .prop_map(|(ip, port)| SocketAddr::from((ip, port))),
        (any::<[u8; 16]>(), any::<u16>())
            .prop_map(|(ip, port)| SocketAddr::from((ip, port))),
    ]
}

fn address() -> impl Strategy<Value = Address> {
    prop_oneof![
        socket().prop_map(Address::Udp),
        socket().prop_map(Address::Tcp),
        (any::<[u8; 32]>(), socket()).prop_filter_map(
            "the key is of small order",
            |(node, at)| Some(Address::Relay {
                node: PublicKey::new(node).ok()?,
                at,
            })
        ),
    ]
}

fn reply() -> impl Strategy<Value = Reply> {
    let range = (any::<u64>(), any::<u32>())
        .prop_map(|(seq, count)| frame::Range { seq, count });
    prop_oneof![
        range
            .clone()
            .prop_map(|range| Reply::Outcome(Outcome::Taken { range })),
        range.prop_map(|range| Reply::Outcome(Outcome::Lost { range })),
        (0..REFUSALS.len())
            .prop_map(|at| Reply::Outcome(Outcome::Refused(REFUSALS[at].0))),
        any::<u64>().prop_map(|through| Reply::Stored { through }),
    ]
}

proptest! {
    #[test]
    fn round_trips_a_write(length in 0..=OPEN_BYTES_MAX, signature: [u8; 32]) {
        let mut write = write(length);
        write.signature[32..].copy_from_slice(&signature);
        prop_assert_eq!(Write::decode(&encode_write(write)), Ok(write));
        prop_assert_eq!(First::decode(&encode_write(write)), Ok(First::Write(write)));
    }

    #[test]
    fn round_trips_an_open(
        authority: u8,
        lease in proptest::option::of(any::<i64>()),
        channels in proptest::collection::vec(name(), 1..8),
    ) {
        let open = Open {
            authority: Authority(authority),
            lease: lease.map(Span::from_nanos),
            channels,
        };
        prop_assert_eq!(Open::decode(&encode_open(&open)), Ok(open));
    }

    #[test]
    fn round_trips_an_away(
        node: u128,
        key in any::<[u8; 32]>().prop_filter_map("small order", |key| PublicKey::new(key).ok()),
        addresses in proptest::collection::vec(address(), 0..=ADDRESSES_MAX),
    ) {
        let away = Away {
            node: node::Key::from_u128(node),
            public_key: key,
            addresses,
        };
        prop_assert_eq!(Away::decode(&encode_away(&away)), Ok(away));
    }

    #[test]
    fn round_trips_a_reply(reply in reply()) {
        prop_assert_eq!(Reply::decode(&encode_reply(reply)), Ok(reply));
    }

    #[test]
    fn refuses_each_address_count_over_the_cap(count in 33..=u8::MAX) {
        prop_assert_eq!(
            Away::decode(&away_with(count, &[])),
            Err(Error::Addresses { count: usize::from(count), cap: ADDRESSES_MAX })
        );
    }
}
