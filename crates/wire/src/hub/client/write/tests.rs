use proptest::prelude::*;
use types::name;

use super::*;
use crate::hub::client::First;

fn write(length: u64) -> Write {
    Write {
        length,
        signature: [0x5e; 64],
    }
}

fn open(lease: Option<Span>, names: &[&str]) -> Open {
    Open {
        authority: Authority(3),
        lease,
        channels: names.iter().map(|name| name.parse().unwrap()).collect(),
    }
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

fn head(time: Time, form: Form, count: u32) -> Head {
    Head {
        time,
        form,
        count,
        series: 2,
    }
}

fn encode_head(head: Head) -> Vec<u8> {
    let mut out = vec![0xaa; Head::LEN];
    head.encode(&mut out);
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
    fn the_head() {
        let mut head = head(Time::Frame(Path::Live), Form::Raw, 0x0102);
        head.series = 0x0304;
        let mut bytes = vec![7, 0, 0, 0x02, 0x01, 0, 0, 0x04, 0x03, 0, 0];
        assert_eq!(encode_head(head), bytes);
        head.time = Time::Frame(Path::Backfill);
        head.form = Form::Encoded;
        bytes[1..3].copy_from_slice(&[1, 1]);
        assert_eq!(encode_head(head), bytes);
        let node = Head {
            time: Time::Node,
            form: Form::Raw,
            count: 1,
            series: 3,
        };
        assert_eq!(encode_head(node), [7, 2, 0, 1, 0, 0, 0, 3, 0, 0, 0]);
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

mod head {
    use super::*;

    fn bytes(time: u8, form: u8, count: u32, series: u32) -> Vec<u8> {
        let mut bytes = vec![7, time, form];
        bytes.extend(count.to_le_bytes());
        bytes.extend(series.to_le_bytes());
        bytes
    }

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|&kind| kind != 7) {
            assert_eq!(
                Head::decode(&zeros(kind, Head::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(Head::decode(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for len in (1..=Head::LEN + 1).filter(|&len| len != Head::LEN) {
            assert_eq!(Head::decode(&zeros(7, len)), Err(Error::Length { len }));
        }
    }

    #[test]
    fn refuses_a_time_that_no_time_has() {
        for time in 3..=u8::MAX {
            assert_eq!(
                Head::decode(&bytes(time, 0, 1, 1)),
                Err(Error::Time { time })
            );
        }
    }

    #[test]
    fn refuses_a_form_that_no_form_has() {
        for form in 2..=u8::MAX {
            assert_eq!(
                Head::decode(&bytes(0, form, 1, 1)),
                Err(Error::Form { form })
            );
        }
    }

    #[test]
    fn refuses_no_samples_and_no_series() {
        assert_eq!(Head::decode(&bytes(0, 0, 0, 1)), Err(Error::Count));
        assert_eq!(Head::decode(&bytes(0, 0, 1, 0)), Err(Error::Series));
    }

    #[test]
    fn refuses_a_node_head_that_is_encoded_or_holds_more_than_one_sample() {
        assert_eq!(
            Head::decode(&bytes(2, 1, 1, 1)),
            Err(Error::Untimed {
                form: Form::Encoded,
                count: 1
            })
        );
        assert_eq!(
            Head::decode(&bytes(2, 0, 2, 1)),
            Err(Error::Untimed {
                form: Form::Raw,
                count: 2
            })
        );
        assert_eq!(
            Head::decode(&bytes(2, 0, 1, 1)),
            Ok(head(Time::Node, Form::Raw, 1)).map(|head| Head { series: 1, ..head })
        );
    }

    #[test]
    fn gives_the_first_defect_in_the_order_of_the_bytes() {
        assert_eq!(
            Head::decode(&bytes(3, 2, 0, 0)),
            Err(Error::Time { time: 3 })
        );
        assert_eq!(
            Head::decode(&bytes(2, 2, 0, 0)),
            Err(Error::Form { form: 2 })
        );
        assert_eq!(Head::decode(&bytes(2, 1, 0, 0)), Err(Error::Count));
        assert_eq!(Head::decode(&bytes(2, 1, 2, 0)), Err(Error::Series));
    }

    #[test]
    #[should_panic(expected = "a head has 0 samples of 2 series, not at least 1 of 1")]
    fn panics_on_an_encode_with_no_samples() {
        encode_head(head(Time::Frame(Path::Live), Form::Raw, 0));
    }

    #[test]
    #[should_panic(expected = "a head has 1 samples of 0 series, not at least 1 of 1")]
    fn panics_on_an_encode_with_no_series() {
        let head = head(Time::Frame(Path::Live), Form::Raw, 1);
        encode_head(Head { series: 0, ..head });
    }

    #[test]
    #[should_panic(
        expected = "a head that the node stamps has 1 samples in the Encoded form"
    )]
    fn panics_on_an_encode_of_an_encoded_node_head() {
        encode_head(head(Time::Node, Form::Encoded, 1));
    }

    #[test]
    #[should_panic(
        expected = "a head that the node stamps has 2 samples in the Raw form"
    )]
    fn panics_on_an_encode_of_a_node_head_of_two_samples() {
        encode_head(head(Time::Node, Form::Raw, 2));
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

mod opening {
    use super::*;
    use crate::hub::client::place;
    use types::channel;

    fn pt(key: u128, data_type: &str) -> Place {
        Place {
            key: channel::Key::from_u128(key),
            data_type: data_type.parse().unwrap(),
            name: "a".parse().unwrap(),
        }
    }

    fn encode_places(places: Places) -> Vec<u8> {
        let mut out = vec![0xaa; Places::LEN];
        places.encode(&mut out);
        out
    }

    fn messages(places: &[Place], max: usize) -> Vec<Vec<u8>> {
        place::messages(places, max)
            .map(|message| {
                let mut out = vec![0xaa; message.encoded_len()];
                message.encode(&mut out);
                out
            })
            .collect()
    }

    /// The `(key, sample type, slot)` of each entry of `start`'s key set, and its
    /// places.
    fn home(start: Option<Start>) -> (Vec<Place>, Vec<(channel::Key, String, u32)>) {
        let Some(Start { places, set }) = start else {
            panic!("the write gave no start");
        };
        let entries = set
            .entries()
            .iter()
            .map(|entry| (entry.key, entry.data_type.to_string(), entry.slot.get()))
            .collect();
        (places, entries)
    }

    #[test]
    fn gives_the_places_and_their_key_set_after_the_run() {
        let run = [pt(1, "u8"), pt(2, "timestamp"), pt(3, "u8")];
        let messages = messages(&run, 49);
        assert_eq!(messages.len(), 2);
        let mut opening = Opening::default();
        let places = Places { count: 3, index: 1 };
        assert!(matches!(
            opening.decode(Some(&encode_places(places))),
            Ok(None)
        ));
        assert!(matches!(opening.decode(Some(&messages[0])), Ok(None)));
        let (places, entries) = home(opening.decode(Some(&messages[1])).unwrap());
        assert_eq!(places, run);
        assert_eq!(
            entries,
            [
                (channel::Key::from_u128(1), "u8".to_owned(), 0),
                (channel::Key::from_u128(2), "timestamp".to_owned(), 1),
                (channel::Key::from_u128(3), "u8".to_owned(), 2),
            ]
        );
    }

    #[test]
    #[should_panic(
        expected = "a write stream is decoded after the decoder gave its start"
    )]
    fn panics_on_a_decode_after_the_places() {
        let mut opening = Opening::default();
        let places = Places { count: 1, index: 0 };
        opening.decode(Some(&encode_places(places))).unwrap();
        opening
            .decode(Some(&messages(&[pt(1, "timestamp")], 300)[0]))
            .unwrap();
        drop(opening.decode(None));
    }

    #[test]
    fn gives_finished_when_the_node_finishes_before_the_last_place() {
        assert!(matches!(
            Opening::default().decode(None),
            Err(Error::Finished)
        ));
        let mut opening = Opening::default();
        let places = Places { count: 2, index: 0 };
        opening.decode(Some(&encode_places(places))).unwrap();
        opening
            .decode(Some(&messages(&[pt(1, "timestamp")], 300)[0]))
            .unwrap();
        assert!(matches!(opening.decode(None), Err(Error::Finished)));
    }

    #[test]
    fn gives_the_errors_of_a_first_message_and_stays_as_it_was() {
        let mut opening = Opening::default();
        assert!(matches!(
            opening.decode(Some(&zeros(5, Places::LEN))),
            Err(Error::Kind { kind: 5 })
        ));
        assert!(matches!(
            opening.decode(Some(&zeros(7, Places::LEN))),
            Err(Error::Kind { kind: 7 })
        ));
        assert!(matches!(
            opening.decode(Some(&[6])),
            Err(Error::Length { len: 1 })
        ));
        assert!(matches!(opening.decode(Some(&[])), Err(Error::Empty)));
        let places = Places { count: 1, index: 0 };
        assert!(matches!(
            opening.decode(Some(&encode_places(places))),
            Ok(None)
        ));
    }

    #[test]
    fn gives_the_errors_of_the_run() {
        let mut opening = Opening::default();
        let places = Places { count: 2, index: 0 };
        opening.decode(Some(&encode_places(places))).unwrap();
        assert!(matches!(
            opening.decode(Some(&messages(&[pt(1, "u8")], 300)[0])),
            Err(Error::Unstamped { .. })
        ));
        let repeated = messages(&[pt(1, "timestamp"), pt(1, "u8")], 300);
        assert!(matches!(
            opening.decode(Some(&repeated[0])),
            Err(Error::Repeated { key }) if key == channel::Key::from_u128(1)
        ));
        let run = [pt(1, "timestamp"), pt(2, "u8")];
        let (places, _) = home(opening.decode(Some(&messages(&run, 300)[0])).unwrap());
        assert_eq!(places, run);
    }
}

fn name() -> impl Strategy<Value = Name> {
    "[a-z][a-z0-9_]{0,6}(\\.[a-z0-9_]{1,6}){0,2}".prop_map(|text| text.parse().unwrap())
}

fn head_strategy() -> impl Strategy<Value = Head> {
    let time = prop_oneof![
        Just(Time::Frame(Path::Live)),
        Just(Time::Frame(Path::Backfill)),
    ];
    let form = prop_oneof![Just(Form::Raw), Just(Form::Encoded)];
    prop_oneof![
        (time, form, 1..=u32::MAX, 1..=u32::MAX).prop_map(
            |(time, form, count, series)| {
                Head {
                    time,
                    form,
                    count,
                    series,
                }
            }
        ),
        (1..=u32::MAX).prop_map(|series| Head {
            time: Time::Node,
            form: Form::Raw,
            count: 1,
            series,
        }),
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
    fn round_trips_a_head(head in head_strategy()) {
        prop_assert_eq!(Head::decode(&encode_head(head)), Ok(head));
    }

    #[test]
    fn round_trips_a_reply(reply in reply()) {
        prop_assert_eq!(Reply::decode(&encode_reply(reply)), Ok(reply));
    }
}
