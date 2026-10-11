use proptest::prelude::*;
use types::sample::{Scalar, Sides};

use super::*;
use crate::hub::client::First;
use crate::hub::client::Request;

fn read(length: u64) -> Read {
    Read {
        length,
        signature: [0x5e; 64],
    }
}

fn open(mode: Mode, patterns: &[&str]) -> Open {
    Open {
        mode,
        select: Selector::new(patterns.iter().copied()).unwrap(),
    }
}

fn pt(key: u128, sample: &str, name: &str) -> Place {
    Place {
        key: channel::Key::from_u128(key),
        sample: sample.parse().unwrap(),
        name: name.parse().unwrap(),
    }
}

fn encode_read(read: Read) -> Vec<u8> {
    let mut out = vec![0xaa; Read::LEN];
    read.encode(&mut out);
    out
}

fn encode_open(open: &Open) -> Vec<u8> {
    let mut out = vec![0xaa; open.encoded_len()];
    open.encode(&mut out);
    out
}

fn encode_places(places: Places) -> Vec<u8> {
    let mut out = vec![0xaa; Places::LEN];
    places.encode(&mut out);
    out
}

fn encode_run(places: &[Place]) -> Vec<u8> {
    let mut out = vec![0xaa; place::encoded_len(places)];
    place::encode(places, &mut out);
    out
}

/// `kind`, then `len - 1` zeros.
fn zeros(kind: u8, len: usize) -> Vec<u8> {
    let mut bytes = vec![0; len];
    bytes[0] = kind;
    bytes
}

mod pins {
    use super::*;

    #[test]
    fn the_read() {
        let mut bytes = vec![6, 0x03, 0x02, 0, 0, 0, 0, 0, 0];
        bytes.extend([0x5e; 64]);
        assert_eq!(encode_read(read(0x0203)), bytes);
    }

    #[test]
    fn the_open() {
        let open = open(Mode::Complete, &["site_a.*", "!site_a.x"]);
        let mut bytes = vec![2, 8, 0];
        bytes.extend(b"site_a.*");
        bytes.extend([9, 0]);
        bytes.extend(b"!site_a.x");
        assert_eq!(open.encoded_len(), 22);
        assert_eq!(encode_open(&open), bytes);
        let latest = super::open(Mode::Latest, &["a"]);
        assert_eq!(encode_open(&latest), [1, 1, 0, b'a']);
    }

    #[test]
    fn the_places() {
        let places = Places { count: 3, index: 2 };
        assert_eq!(encode_places(places), [6, 3, 0, 0, 0, 2, 0, 0, 0]);
    }

    #[test]
    fn the_place() {
        let place = pt(0x0102, "f32[3]", "site_a.pt");
        let mut bytes = vec![0x02, 0x01];
        bytes.extend([0; 14]);
        bytes.push(6);
        bytes.extend(b"f32[3]");
        bytes.push(9);
        bytes.extend(b"site_a.pt");
        assert_eq!(encode_run(&[place]), bytes);
    }
}

mod first {
    use super::*;

    #[test]
    fn decodes_a_request_and_a_read() {
        let request = Request {
            length: 4,
            signature: [0x5e; 64],
        };
        let mut bytes = vec![0; Request::LEN];
        request.encode(&mut bytes);
        assert_eq!(First::decode(&bytes), Ok(First::Request(request)));
        assert_eq!(
            First::decode(&encode_read(read(4))),
            Ok(First::Read(read(4)))
        );
    }

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|kind| ![5, 6].contains(kind)) {
            assert_eq!(
                First::decode(&zeros(kind, Read::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(First::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn gives_the_error_of_the_kind_that_it_names() {
        assert_eq!(First::decode(&[5]), Err(Error::Length { len: 1 }));
        assert_eq!(First::decode(&[6]), Err(Error::Length { len: 1 }));
        let mut over = encode_read(read(1));
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

mod read {
    use super::*;

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|&kind| kind != 6) {
            assert_eq!(
                Read::decode(&zeros(kind, Read::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Read::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for len in [1, 9, 72, 74] {
            assert_eq!(Read::decode(&zeros(6, len)), Err(Error::Length { len }));
        }
    }

    #[test]
    fn checks_the_length_before_the_cap() {
        let mut long = zeros(6, Read::LEN + 1);
        long[1..9].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(Read::decode(&long), Err(Error::Length { len: 74 }));
    }

    #[test]
    fn takes_an_open_at_the_cap_and_refuses_one_over_it() {
        let at = encode_read(read(OPEN_BYTES_MAX));
        assert_eq!(Read::decode(&at), Ok(read(OPEN_BYTES_MAX)));
        let mut over = at;
        over[1..9].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            Read::decode(&over),
            Err(Error::Oversize {
                length: u64::MAX,
                cap: OPEN_BYTES_MAX,
            })
        );
    }

    #[test]
    fn gives_its_body() {
        assert_eq!(read(5).body().remain(), 5);
        assert_eq!(read(0).body().remain(), 0);
    }
}

mod open {
    use super::*;

    #[test]
    fn refuses_an_empty_open() {
        assert_eq!(Open::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_a_mode_that_is_neither_latest_nor_complete() {
        for kind in (0..=u8::MAX).filter(|kind| ![1, 2].contains(kind)) {
            assert_eq!(Open::decode(&[kind, 1, 0, b'a']), Err(Error::Kind { kind }));
        }
    }

    #[test]
    fn refuses_a_pattern_that_ends_past_the_open() {
        assert_eq!(Open::decode(&[1, 1]), Err(Error::Length { len: 2 }));
        assert_eq!(
            Open::decode(&[1, 2, 0, b'a']),
            Err(Error::Length { len: 4 })
        );
    }

    #[test]
    fn refuses_an_open_with_no_include() {
        assert_eq!(Open::decode(&[1]), Err(Error::Selector));
        assert_eq!(Open::decode(&[1, 2, 0, b'!', b'a']), Err(Error::Selector));
    }

    #[test]
    fn refuses_a_pattern_that_does_not_read() {
        assert_eq!(Open::decode(&[1, 0, 0]), Err(Error::Selector));
        assert_eq!(
            Open::decode(&[1, 3, 0, b'a', b'*', b'b']),
            Err(Error::Selector)
        );
        assert_eq!(Open::decode(&[1, 1, 0, 0xff]), Err(Error::Selector));
    }

    #[test]
    fn decodes_an_exclusion_as_written() {
        let decoded = Open::decode(&[2, 1, 0, b'a', 2, 0, b'!', b'b']);
        assert_eq!(decoded, Ok(open(Mode::Complete, &["a", "!b"])));
    }
}

mod places {
    use super::*;

    #[test]
    fn decodes_places_with_the_index_at_each_place() {
        for index in 0..3 {
            let places = Places { count: 3, index };
            assert_eq!(Places::decode(&encode_places(places)), Ok(places));
        }
    }

    #[test]
    fn refuses_an_index_that_is_not_below_the_count() {
        let mut bytes = encode_places(Places { count: 3, index: 0 });
        bytes[5] = 3;
        assert_eq!(
            Places::decode(&bytes),
            Err(Error::Index { index: 3, count: 3 })
        );
        assert_eq!(
            Places::decode(&zeros(6, Places::LEN)),
            Err(Error::Index { index: 0, count: 0 })
        );
    }

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|&kind| kind != 6) {
            assert_eq!(
                Places::decode(&zeros(kind, Places::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Places::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for len in [1, 8, 10] {
            assert_eq!(Places::decode(&zeros(6, len)), Err(Error::Length { len }));
        }
    }
}

mod run {
    use super::*;

    #[test]
    fn decodes_each_place_of_a_message() {
        let places = [pt(1, "timestamp", "a.time"), pt(2, "list<u8, 16>", "a.b")];
        assert_eq!(place::decode(&encode_run(&places)), Ok(places.to_vec()));
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(place::decode(&[]), Err(Error::Empty));
    }

    /// Each place of `u8` named `a` is 21 bytes.
    #[test]
    fn cuts_a_run_into_messages_of_as_many_whole_places_as_fit() {
        let places = [pt(1, "u8", "a"), pt(2, "u8", "a"), pt(3, "u8", "a")];
        let cut = |max| {
            place::messages(&places, max)
                .map(<[Place]>::len)
                .collect::<Vec<_>>()
        };
        assert_eq!(cut(21), vec![1, 1, 1]);
        assert_eq!(cut(41), vec![1, 1, 1]);
        assert_eq!(cut(42), vec![2, 1]);
        assert_eq!(cut(63), vec![3]);
        assert_eq!(place::messages(&[], 0).count(), 0);
    }

    #[test]
    fn refuses_a_message_that_ends_inside_a_place() {
        let bytes = encode_run(&[pt(1, "u8", "a")]);
        for len in 1..bytes.len() {
            assert_eq!(
                place::decode(&bytes[..len]),
                Err(Error::Length { len }),
                "{len}"
            );
        }
    }

    #[test]
    fn refuses_a_sample_type_or_a_name_that_does_not_read() {
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[17] = b'x';
        assert_eq!(place::decode(&bytes), Err(Error::Place));
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[20] = b'*';
        assert_eq!(place::decode(&bytes), Err(Error::Place));
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[20] = 0xff;
        assert_eq!(place::decode(&bytes), Err(Error::Place));
    }
}

#[test]
#[should_panic(expected = "an open of 65537 bytes is over the cap of 65536")]
fn panics_on_a_read_over_the_cap() {
    read(OPEN_BYTES_MAX + 1).encode(&mut [0; Read::LEN]);
}

#[test]
#[should_panic(expected = "the index is at place 2 of 2")]
fn panics_on_places_with_the_index_at_the_count() {
    Places { count: 2, index: 2 }.encode(&mut [0; Places::LEN]);
}

#[test]
#[should_panic(expected = "a place of 21 bytes is over a message of 20")]
fn panics_on_a_place_over_a_message() {
    place::messages(&[pt(1, "u8", "a")], 20).for_each(drop);
}

#[test]
#[should_panic(expected = "a message of a run holds at least one item")]
fn panics_on_a_message_of_no_place() {
    place::encode(&[], &mut []);
}

fn selector() -> impl Strategy<Value = Selector> {
    let segment = prop_oneof![Just("*".to_owned()), "[a-z0-9_]{1,6}"];
    let pattern = (any::<bool>(), proptest::collection::vec(segment, 1..4)).prop_map(
        |(excluded, segments)| {
            let text = segments.join(".");
            if excluded { format!("!{text}") } else { text }
        },
    );
    proptest::collection::vec(pattern, 1..6)
        .prop_filter_map("the selector does not read", |patterns| {
            Selector::new(patterns.iter().map(String::as_str)).ok()
        })
}

fn sample() -> impl Strategy<Value = Type> {
    let element = prop_oneof![
        Just(Scalar::Bool),
        Just(Scalar::U8),
        Just(Scalar::F64),
        Just(Scalar::Stamp),
        Just(Scalar::Span),
        Just(Scalar::Uuid),
    ];
    (element, any::<u32>(), any::<u16>(), 0..6_u8).prop_map(
        |(element, n, side, form)| match form {
            0 => Type::Scalar(element),
            1 => Type::Array { element, len: n },
            2 => Type::List { element, max: n },
            3 => Type::String,
            4 => Type::Bytes,
            _ => Type::Matrix {
                element,
                sides: Sides {
                    rows: side,
                    columns: side,
                },
            },
        },
    )
}

fn places() -> impl Strategy<Value = Vec<Place>> {
    let place = (
        any::<u128>(),
        sample(),
        "[a-z_]{1,8}(\\.[a-z0-9_]{1,8}){0,3}",
    )
        .prop_map(|(key, sample, name)| Place {
            key: channel::Key::from_u128(key),
            sample,
            name: name.parse().unwrap(),
        });
    proptest::collection::vec(place, 1..8)
}

proptest! {
    #[test]
    fn round_trips_an_open(select in selector(), complete in any::<bool>()) {
        let mode = if complete { Mode::Complete } else { Mode::Latest };
        let open = Open { mode, select };
        prop_assert_eq!(Open::decode(&encode_open(&open)), Ok(open));
    }

    #[test]
    fn round_trips_a_read(length in 0..=OPEN_BYTES_MAX, signature in any::<[u8; 32]>()) {
        let mut read = read(length);
        read.signature[32..].copy_from_slice(&signature);
        prop_assert_eq!(Read::decode(&encode_read(read)), Ok(read));
        prop_assert_eq!(First::decode(&encode_read(read)), Ok(First::Read(read)));
    }

    #[test]
    fn round_trips_places((count, index) in (1..=u32::MAX).prop_flat_map(|count| (Just(count), 0..count))) {
        let places = Places { count, index };
        prop_assert_eq!(Places::decode(&encode_places(places)), Ok(places));
    }

    #[test]
    fn decodes_a_run_cut_into_messages_of_whole_places(
        places in places(),
        sizes in proptest::collection::vec(1..=3_usize, 1..4),
    ) {
        let mut decoded = Vec::new();
        let mut rest = places.as_slice();
        for size in sizes.into_iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let (message, after) = rest.split_at(size.min(rest.len()));
            decoded.extend(place::decode(&encode_run(message))?);
            rest = after;
        }
        prop_assert_eq!(decoded, places);
    }

    #[test]
    fn cuts_a_run_into_full_messages_under_the_max(places in places(), max in 100..300_usize) {
        let messages: Vec<_> = place::messages(&places, max).collect();
        for message in &messages {
            prop_assert!(place::encoded_len(message) <= max);
        }
        for (message, next) in messages.iter().zip(messages.iter().skip(1)) {
            let grown = [*message, &next[..1]].concat();
            prop_assert!(place::encoded_len(&grown) > max);
        }
        prop_assert_eq!(messages.concat(), places);
    }
}
