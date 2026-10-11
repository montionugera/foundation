use proptest::prelude::*;
use types::name;
use types::sample::{self, Scalar, Sides};

use super::*;
use crate::hub::client::First;
use crate::hub::client::Request;
use crate::hub::text;

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

fn pt(key: u128, data_type: &str, name: &str) -> Place {
    Place {
        key: channel::Key::from_u128(key),
        data_type: data_type.parse().unwrap(),
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

fn not_utf8<E>(bytes: &[u8]) -> text::Error<E> {
    text::Error::Utf8(std::str::from_utf8(bytes).unwrap_err())
}

fn unselected(error: name::Error) -> Error {
    Error::Selector(text::Error::Parse(Box::new(error)))
}

/// `places` as one message of the run.
fn encode_run(places: &[Place]) -> Vec<u8> {
    let [message] = place::messages(places, usize::MAX).collect::<Vec<_>>()[..] else {
        panic!("{} places are one message", places.len());
    };
    encode_message(message)
}

fn encode_message(message: place::Message<'_>) -> Vec<u8> {
    let mut out = vec![0xaa; message.encoded_len()];
    message.encode(&mut out);
    out
}

/// `message` as the first message of a read stream.
fn decode_places(message: &[u8]) -> Result<Places, Error> {
    match Reader::new(Mode::Latest).decode(message)? {
        FromNode::Places(places) => Ok(places),
        other => panic!("the first message gave {other:?}"),
    }
}

/// `message` as a message of a run long enough for any test, before its index place.
fn decode_run(message: &[u8]) -> Result<Vec<Place>, Error> {
    let mut reader = Reader::new(Mode::Latest);
    let places = Places {
        count: u32::MAX,
        index: u32::MAX - 1,
    };
    reader.decode(&encode_places(places))?;
    match reader.decode(message)? {
        FromNode::Run(run) => Ok(run),
        other => panic!("a message of the run gave {other:?}"),
    }
}

/// The places that `message` encodes.
fn decode_message(message: place::Message<'_>) -> Vec<Place> {
    decode_run(&encode_message(message)).expect("a message of the run decodes")
}

/// The bytes of `place` in a message of the run.
fn place_len(place: &Place) -> usize {
    let [message] = place::messages(std::slice::from_ref(place), usize::MAX)
        .collect::<Vec<_>>()[..]
    else {
        panic!("a place is one message");
    };
    message.encoded_len()
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
        for byte in (0..=u8::MAX).filter(|byte| ![1, 2].contains(byte)) {
            assert_eq!(Open::decode(&[byte, 1, 0, b'a']), Err(Error::Mode { byte }));
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
        assert_eq!(Open::decode(&[1]), Err(unselected(name::Error::NoInclude)));
        assert_eq!(
            Open::decode(&[1, 2, 0, b'!', b'a']),
            Err(unselected(name::Error::NoInclude))
        );
    }

    #[test]
    fn refuses_a_pattern_that_does_not_read() {
        assert_eq!(
            Open::decode(&[1, 0, 0]),
            Err(unselected(name::Error::Empty))
        );
        assert_eq!(
            Open::decode(&[1, 3, 0, b'a', b'*', b'b']),
            Err(unselected(name::Error::Wildcard {
                input: "a*b".into()
            }))
        );
        assert_eq!(
            Open::decode(&[1, 1, 0, 0xff]),
            Err(Error::Selector(not_utf8(&[0xff])))
        );
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
            assert_eq!(decode_places(&encode_places(places)), Ok(places));
        }
    }

    #[test]
    fn refuses_an_index_that_is_not_below_the_count() {
        let mut bytes = encode_places(Places { count: 3, index: 0 });
        bytes[5] = 3;
        assert_eq!(
            decode_places(&bytes),
            Err(Error::Index { index: 3, count: 3 })
        );
        assert_eq!(
            decode_places(&zeros(6, Places::LEN)),
            Err(Error::Index { index: 0, count: 0 })
        );
    }

    #[test]
    fn refuses_each_other_kind() {
        for kind in (0..=u8::MAX).filter(|&kind| kind != 6) {
            assert_eq!(
                decode_places(&zeros(kind, Places::LEN)),
                Err(Error::Kind { kind })
            );
        }
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(decode_places(&[]), Err(Error::Empty));
    }

    #[test]
    fn refuses_each_wrong_length() {
        for len in [1, 8, 10] {
            assert_eq!(decode_places(&zeros(6, len)), Err(Error::Length { len }));
        }
    }
}

mod run {
    use super::*;

    #[test]
    fn decodes_each_place_of_a_message() {
        let places = [pt(1, "timestamp", "a.time"), pt(2, "list<u8, 16>", "a.b")];
        assert_eq!(decode_run(&encode_run(&places)), Ok(places.to_vec()));
    }

    #[test]
    fn refuses_an_empty_message() {
        assert_eq!(decode_run(&[]), Err(Error::Empty));
    }

    /// Each place of `u8` named `a` is 21 bytes.
    #[test]
    fn cuts_a_run_into_messages_of_as_many_whole_places_as_fit() {
        let places = [pt(1, "u8", "a"), pt(2, "u8", "a"), pt(3, "u8", "a")];
        let cut = |max| {
            place::messages(&places, max)
                .map(|message| decode_message(message).len())
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
                decode_run(&bytes[..len]),
                Err(Error::Length { len }),
                "{len}"
            );
        }
    }

    #[test]
    fn refuses_a_sample_type_or_a_name_that_does_not_read() {
        let cases: [(usize, u8, Error); 4] = [
            (
                17,
                b'x',
                Error::Sample(text::Error::Parse(sample::Error::Syntax)),
            ),
            (17, 0xff, Error::Sample(not_utf8(&[0xff, b'8']))),
            (
                20,
                b'*',
                Error::Name(text::Error::Parse(Box::new(name::Error::Wildcard {
                    input: "*".into(),
                }))),
            ),
            (20, 0xff, Error::Name(not_utf8(&[0xff]))),
        ];
        for (at, byte, error) in cases {
            let mut bytes = encode_run(&[pt(1, "u8", "a")]);
            bytes[at] = byte;
            assert_eq!(decode_run(&bytes), Err(error), "{at} {byte}");
        }
    }

    #[test]
    fn gives_the_first_defect_in_the_order_of_the_bytes() {
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[17] = b'x';
        bytes[19] = 5;
        assert_eq!(
            decode_run(&bytes),
            Err(Error::Sample(text::Error::Parse(sample::Error::Syntax)))
        );
    }

    #[test]
    fn gives_a_bad_sample_type_before_a_missing_name() {
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[17] = b'x';
        bytes.truncate(19);
        assert_eq!(
            decode_run(&bytes),
            Err(Error::Sample(text::Error::Parse(sample::Error::Syntax)))
        );
    }

    #[test]
    fn gives_a_bad_name_before_a_cut_place_after_it() {
        let mut bytes = encode_run(&[pt(1, "u8", "a")]);
        bytes[20] = 0xff;
        bytes.extend_from_slice(&[0; 3]);
        assert_eq!(decode_run(&bytes), Err(Error::Name(not_utf8(&[0xff]))));
    }
}

mod reader {
    use super::*;
    use crate::hub::tests::encode_reply;
    use crate::hub::{FromHome, Head, Reply};
    use types::frame::{Path, Range};

    const OPENED: u8 = 1;
    const BEHIND: u8 = 3;

    /// A place of `u8` named `a`: 21 bytes.
    fn byte(key: u128) -> Place {
        pt(key, "u8", "a")
    }

    /// A place of `timestamp` named `a`: 28 bytes.
    fn stamp(key: u128) -> Place {
        pt(key, "timestamp", "a")
    }

    /// `places` cut into messages of the run of at most `max` bytes.
    fn messages(places: &[Place], max: usize) -> Vec<Vec<u8>> {
        place::messages(places, max).map(encode_message).collect()
    }

    fn head(series: u32) -> Vec<u8> {
        encode_reply(Reply::Head(Head {
            path: Path::Live,
            range: Range { seq: 0, count: 1 },
            series,
        }))
    }

    /// A reader of `mode` that took `places`.
    fn in_run(mode: Mode, places: Places) -> Reader {
        let mut reader = Reader::new(mode);
        reader
            .decode(&encode_places(places))
            .expect("the places decode");
        reader
    }

    /// A reader of `mode` that took `places` and the whole run of `run`.
    fn after_run(mode: Mode, places: Places, run: &[Place]) -> Reader {
        let mut reader = in_run(mode, places);
        reader.decode(&encode_run(run)).expect("the run decodes");
        reader
    }

    #[derive(Debug, PartialEq)]
    enum Event {
        Places(Places),
        Run(Vec<Place>),
        Opened,
        Head(u32),
        Behind,
        Body(Vec<u8>),
    }

    fn event(reader: &mut Reader, message: &[u8]) -> Result<Event, Error> {
        reader.decode(message).map(|event| match event {
            FromNode::Places(places) => Event::Places(places),
            FromNode::Run(run) => Event::Run(run),
            FromNode::Home(FromHome::Opened) => Event::Opened,
            FromNode::Home(FromHome::Head(head)) => Event::Head(head.series),
            FromNode::Home(FromHome::Behind) => Event::Behind,
            FromNode::Home(FromHome::Body { bytes, .. }) => Event::Body(bytes.to_vec()),
            FromNode::Home(FromHome::Ends { .. }) => panic!("no test sends ends"),
        })
    }

    #[test]
    fn decodes_the_places_then_the_run_then_the_replies() {
        let places = Places { count: 3, index: 1 };
        let run = [byte(1), stamp(2), byte(3)];
        let messages = messages(&run, 49);
        assert_eq!(messages.len(), 2);
        let mut reader = Reader::new(Mode::Complete);
        assert_eq!(reader.body(), None);
        assert_eq!(reader.end(), Err(Error::Finished));
        assert_eq!(
            event(&mut reader, &encode_places(places)),
            Ok(Event::Places(places))
        );
        assert_eq!(reader.body(), None);
        assert_eq!(reader.end(), Err(Error::Finished));
        assert_eq!(
            event(&mut reader, &messages[0]),
            Ok(Event::Run(run[..2].to_vec()))
        );
        assert_eq!(reader.body(), None);
        assert_eq!(reader.end(), Err(Error::Finished));
        assert_eq!(
            event(&mut reader, &messages[1]),
            Ok(Event::Run(run[2..].to_vec()))
        );
        assert_eq!(event(&mut reader, &[OPENED]), Ok(Event::Opened));
        assert_eq!(event(&mut reader, &head(3)), Ok(Event::Head(3)));
    }

    #[test]
    fn decodes_the_first_message_as_places() {
        let mut reader = Reader::new(Mode::Latest);
        assert_eq!(reader.decode(&[]).err(), Some(Error::Empty));
        assert_eq!(
            reader.decode(&[OPENED]).err(),
            Some(Error::Kind { kind: OPENED })
        );
        assert_eq!(
            reader.decode(&encode_run(&[stamp(2)])).err(),
            Some(Error::Kind { kind: 2 })
        );
        assert_eq!(reader.decode(&[6]).err(), Some(Error::Length { len: 1 }));
        assert_eq!(
            reader.decode(&zeros(6, Places::LEN)).err(),
            Some(Error::Index { index: 0, count: 0 })
        );
        let places = Places { count: 1, index: 0 };
        assert_eq!(
            event(&mut reader, &encode_places(places)),
            Ok(Event::Places(places))
        );
    }

    #[test]
    fn reads_a_second_places_or_a_reply_in_the_run_as_run_bytes() {
        let places = Places { count: 1, index: 0 };
        let mut reader = in_run(Mode::Complete, places);
        assert_eq!(
            reader.decode(&encode_places(places)).err(),
            Some(Error::Length { len: 9 })
        );
        assert_eq!(
            reader.decode(&[OPENED]).err(),
            Some(Error::Length { len: 1 })
        );
        assert_eq!(reader.decode(&[]).err(), Some(Error::Empty));
        assert_eq!(
            event(&mut reader, &encode_run(&[stamp(1)])),
            Ok(Event::Run(vec![stamp(1)]))
        );
    }

    #[test]
    fn refuses_a_message_with_more_places_than_remain() {
        let places = Places { count: 2, index: 0 };
        let mut reader = in_run(Mode::Complete, places);
        let three = [stamp(1), byte(2), byte(3)];
        assert_eq!(
            reader.decode(&encode_run(&three)).err(),
            Some(Error::Run {
                items: 3,
                remain: 2
            })
        );
        reader
            .decode(&encode_run(&three[..1]))
            .expect("one place decodes");
        assert_eq!(
            reader.decode(&encode_run(&three[1..])).err(),
            Some(Error::Run {
                items: 2,
                remain: 1
            })
        );
        assert_eq!(
            event(&mut reader, &encode_run(&three[1..2])),
            Ok(Event::Run(vec![byte(2)]))
        );
        assert_eq!(event(&mut reader, &[OPENED]), Ok(Event::Opened));
    }

    #[test]
    fn checks_the_count_before_the_index_place() {
        let places = Places { count: 1, index: 0 };
        let mut reader = in_run(Mode::Complete, places);
        assert_eq!(
            reader.decode(&encode_run(&[byte(1), byte(2)])).err(),
            Some(Error::Run {
                items: 2,
                remain: 1
            })
        );
    }

    /// A run of 4 places in 2 messages of 2, each `u8`: the message that holds the
    /// index place gives `Unstamped`, and each before it decodes.
    #[test]
    fn refuses_an_index_place_that_is_not_a_stamp_in_the_message_that_holds_it() {
        let run = [byte(1), byte(2), byte(3), byte(4)];
        let messages = messages(&run, 42);
        assert_eq!(messages.len(), 2);
        let unstamped = Error::Unstamped {
            sample: "u8".parse().unwrap(),
        };
        for index in 0..4 {
            let mut reader = in_run(Mode::Complete, Places { count: 4, index });
            let held = usize::try_from(index / 2).unwrap();
            for message in &messages[..held] {
                reader
                    .decode(message)
                    .expect("a message before the index decodes");
            }
            assert_eq!(
                reader.decode(&messages[held]).err(),
                Some(unstamped.clone()),
                "{index}"
            );
            assert_eq!(reader.end(), Err(Error::Finished));
        }
    }

    #[test]
    fn takes_an_index_place_of_a_stamp_at_each_place() {
        for index in 0..4 {
            let mut run = [byte(1), byte(2), byte(3), byte(4)];
            run[usize::try_from(index).unwrap()] = stamp(9);
            let mut reader = in_run(Mode::Complete, Places { count: 4, index });
            for message in messages(&run, 49) {
                reader.decode(&message).expect("the run decodes");
            }
            assert_eq!(event(&mut reader, &[OPENED]), Ok(Event::Opened));
        }
    }

    #[test]
    fn refuses_an_array_of_stamps_as_the_index() {
        let mut reader = in_run(Mode::Complete, Places { count: 1, index: 0 });
        let error = reader
            .decode(&encode_run(&[pt(1, "timestamp[2]", "a")]))
            .expect_err("an array is not a stamp");
        assert_eq!(
            error,
            Error::Unstamped {
                sample: "timestamp[2]".parse().unwrap()
            }
        );
        assert_eq!(
            error.to_string(),
            "the index place of the read has sample type timestamp[2], not timestamp"
        );
    }

    #[test]
    fn opens_a_session_of_the_count_of_places() {
        let mut reader = after_run(
            Mode::Complete,
            Places { count: 2, index: 1 },
            &[byte(1), stamp(2)],
        );
        reader.decode(&[OPENED]).expect("the session opens");
        assert_eq!(
            reader.decode(&head(3)).err(),
            Some(Error::Places {
                series: 3,
                places: 2
            })
        );
        assert_eq!(event(&mut reader, &head(2)), Ok(Event::Head(2)));
    }

    #[test]
    fn opens_a_latest_session_for_a_latest_read() {
        let places = Places { count: 1, index: 0 };
        let mut latest = after_run(Mode::Latest, places, &[stamp(1)]);
        latest.decode(&[OPENED]).expect("the session opens");
        assert_eq!(
            latest.decode(&[BEHIND]).err(),
            Some(Error::Latest { kind: BEHIND })
        );
        let mut complete = after_run(Mode::Complete, places, &[stamp(1)]);
        complete.decode(&[OPENED]).expect("the session opens");
        assert_eq!(event(&mut complete, &[BEHIND]), Ok(Event::Behind));
        assert_eq!(complete.end(), Ok(()));
    }

    #[test]
    fn gives_the_body_and_the_end_of_the_session() {
        let places = Places { count: 1, index: 0 };
        let mut reader = after_run(Mode::Complete, places, &[stamp(1)]);
        assert_eq!(reader.decode(&[]).err(), Some(Error::Empty));
        reader.decode(&[OPENED]).expect("the session opens");
        reader.decode(&head(1)).expect("the head decodes");
        reader
            .decode(&crate::hub::tests::encode_ends(&[(0, 5)]))
            .expect("the end decodes");
        assert_eq!(reader.body(), Some(0));
        assert_eq!(reader.end(), Err(Error::Unfinished { remain: 5 }));
        assert_eq!(event(&mut reader, &[7; 2]), Ok(Event::Body(vec![7; 2])));
        assert_eq!(reader.body(), Some(2));
    }
}

#[test]
#[should_panic(expected = "an open of 65537 bytes is over the cap of 65536")]
fn panics_on_a_read_over_the_cap() {
    read(OPEN_BYTES_MAX + 1).encode(&mut [0; Read::LEN]);
}

#[test]
#[should_panic(expected = "an open of 65537 bytes is over the cap of 65536")]
fn panics_on_the_body_of_a_read_over_the_cap() {
    let _body = read(OPEN_BYTES_MAX + 1).body();
}

/// Each sample type at its largest counts. The matches fail to compile when a scalar
/// or a shape is added, so that a new one cannot pass the 300-byte bound unchecked.
fn largest_types() -> Vec<Type> {
    use Scalar::{
        Bool, F32, F64, I8, I16, I32, I64, Span, Stamp, U8, U16, U32, U64, Uuid,
    };
    let scalars = [
        Bool, I8, I16, I32, I64, U8, U16, U32, U64, F32, F64, Stamp, Span, Uuid,
    ];
    for scalar in scalars {
        match scalar {
            Bool | I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Stamp
            | Span | Uuid => {}
        }
    }
    let sides = Sides {
        rows: u16::MAX,
        columns: u16::MAX,
    };
    let mut types = vec![Type::String, Type::Bytes];
    for element in scalars {
        types.extend([
            Type::Scalar(element),
            Type::Array {
                element,
                len: u32::MAX,
            },
            Type::List {
                element,
                max: u32::MAX,
            },
            Type::Matrix { element, sides },
        ]);
    }
    for sample in &types {
        match sample {
            Type::Scalar(_)
            | Type::Array { .. }
            | Type::List { .. }
            | Type::String
            | Type::Bytes
            | Type::Matrix { .. } => {}
        }
    }
    types
}

#[test]
fn fits_the_largest_place_in_300_bytes() {
    let name: Name = "a".repeat(Name::MAX_BYTES).parse().unwrap();
    let places: Vec<_> = largest_types()
        .into_iter()
        .map(|data_type| Place {
            key: channel::Key::from_u128(1),
            data_type,
            name: name.clone(),
        })
        .collect();
    for place in &places {
        let one = std::slice::from_ref(place);
        let runs: Vec<_> = place::messages(one, 300).map(decode_message).collect();
        assert_eq!(runs, [one], "{}", place.data_type);
    }
    assert_eq!(places.iter().map(place_len).max(), Some(300));
}

#[test]
#[should_panic(expected = "out has 22 bytes, and the message has 21")]
fn panics_on_a_message_into_out_of_another_length() {
    let places = [pt(1, "u8", "a")];
    let [message] = place::messages(&places, 21).collect::<Vec<_>>()[..] else {
        panic!("one place is one message");
    };
    message.encode(&mut [0; 22]);
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
        .prop_map(|(key, data_type, name)| Place {
            key: channel::Key::from_u128(key),
            data_type,
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
    fn round_trips_a_read(
        length in 0..=OPEN_BYTES_MAX,
        signature in any::<[u8; 32]>(),
    ) {
        let mut read = read(length);
        read.signature[32..].copy_from_slice(&signature);
        prop_assert_eq!(Read::decode(&encode_read(read)), Ok(read));
        prop_assert_eq!(First::decode(&encode_read(read)), Ok(First::Read(read)));
    }

    #[test]
    fn round_trips_places(
        (count, index) in (1..=u32::MAX)
            .prop_flat_map(|count| (Just(count), 0..count)),
    ) {
        let places = Places { count, index };
        prop_assert_eq!(decode_places(&encode_places(places)), Ok(places));
    }

    /// The reader gives each place of the run, and then the replies. When the index
    /// place is not a stamp, the message that holds it gives `Unstamped`.
    #[test]
    fn decodes_a_run_cut_into_messages_of_whole_places(
        (mut places, index) in places().prop_flat_map(|places| {
            let len = places.len();
            (Just(places), 0..len)
        }),
        max in 100..1_000_usize,
        complete in any::<bool>(),
        stamp in any::<bool>(),
    ) {
        if stamp {
            places[index].data_type = "timestamp".parse().unwrap();
        }
        let index_type = places[index].data_type;
        let stamped = index_type == "timestamp".parse().unwrap();
        let mode = if complete { Mode::Complete } else { Mode::Latest };
        let mut reader = Reader::new(mode);
        let count = u32::try_from(places.len()).unwrap();
        let reply = Places { count, index: u32::try_from(index).unwrap() };
        let first = encode_places(reply);
        let decoded = reader.decode(&first);
        prop_assert!(matches!(decoded, Ok(FromNode::Places(got)) if got == reply));
        let mut decoded = Vec::new();
        for message in place::messages(&places, max) {
            prop_assert_eq!(reader.body(), None);
            prop_assert_eq!(reader.end(), Err(Error::Finished));
            let bytes = encode_message(message);
            let len = decode_run(&bytes)?.len();
            let held = (decoded.len()..decoded.len().saturating_add(len)).contains(&index);
            match reader.decode(&bytes) {
                Ok(FromNode::Run(run)) => decoded.extend(run),
                Err(error) if held && !stamped => {
                    prop_assert_eq!(error, Error::Unstamped { sample: index_type });
                    return Ok(());
                }
                other => prop_assert!(false, "{other:?}"),
            }
        }
        prop_assert!(stamped);
        prop_assert_eq!(&decoded, &places);
        let opened = reader.decode(&[1]);
        prop_assert!(matches!(opened, Ok(FromNode::Home(crate::hub::FromHome::Opened))));
    }

    /// A fault after any message of a cut run gives its exact error: a second
    /// `Places` or a reply reads as run bytes too short for a place, and a message of
    /// one more place than remain gives `Run`.
    #[test]
    fn refuses_a_fault_after_any_message_of_a_run(
        mut places in places(),
        max in 100..1_000_usize,
        at in any::<prop::sample::Index>(),
        fault in 0..3_u8,
    ) {
        places[0].data_type = "timestamp".parse().unwrap();
        let count = u32::try_from(places.len()).unwrap();
        let reply = Places { count, index: 0 };
        let mut reader = Reader::new(Mode::Complete);
        reader.decode(&encode_places(reply))?;
        let messages: Vec<_> = place::messages(&places, max).collect();
        let at = at.index(messages.len());
        let mut sent = 0_usize;
        for message in messages.into_iter().take(at) {
            match reader.decode(&encode_message(message)) {
                Ok(FromNode::Run(run)) => sent = sent.saturating_add(run.len()),
                other => prop_assert!(false, "{other:?}"),
            }
        }
        let (bytes, error) = match fault {
            0 => (encode_places(reply), Error::Length { len: Places::LEN }),
            1 => (vec![1], Error::Length { len: 1 }),
            _ => {
                let mut over = places[sent..].to_vec();
                over.push(places[0].clone());
                let remain = count.saturating_sub(u32::try_from(sent).unwrap());
                (encode_run(&over), Error::Run { items: over.len(), remain })
            }
        };
        prop_assert_eq!(reader.decode(&bytes).err(), Some(error));
    }

    #[test]
    fn cuts_a_run_into_full_messages_under_the_max(
        places in places(),
        max in 100..300_usize,
    ) {
        let messages: Vec<_> = place::messages(&places, max)
            .map(|message| (message.encoded_len(), decode_message(message)))
            .collect();
        for (len, decoded) in &messages {
            prop_assert!(*len <= max);
            prop_assert_eq!(*len, decoded.iter().map(place_len).sum::<usize>());
        }
        for ((len, _), (_, next)) in messages.iter().zip(messages.iter().skip(1)) {
            prop_assert!(len.saturating_add(place_len(&next[0])) > max);
        }
        let cut: Vec<_> =
            messages.into_iter().flat_map(|(_, decoded)| decoded).collect();
        prop_assert_eq!(cut, places);
    }
}
