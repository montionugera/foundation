//! The decoders of `wire::hub::client` never panic, each but `Open::decode` and
//! `Reader::decode` refuses a message of another kind with `Error::Kind`,
//! `Reader::decode` refuses a first message of another kind with it, each message that
//! decodes encodes to its bytes, each body ends at its length and nowhere else, each
//! refusal of a body is the one that its rest gives, a read stream gives with its last
//! place the places of its run, their key set, and the decoder of a session of their
//! count and the mode, a refused message of a read stream leaves its decoder as it
//! was, and each valid message made from the input reads back.
//!
//! Input: the messages of one stream (`fuzz::messages`). Each decoder reads each
//! message, an open among them; the body of the first message, when it is a request, a
//! response, or a read, takes each later message; and a `client::Reader` of each mode
//! takes each message until it gives the last place, and then the end of the stream.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::{
    arbitrary::{self, Unstructured},
    fuzz_target,
};
use types::channel;
use types::connection;
use types::ed25519::PublicKey;
use types::hello::Hello;
use types::name::Selector;
use types::node;
use types::sample::{Scalar, Type};
use types::time::{Interval, Stamp};
use wire::hub;
use wire::hub::client::{
    BODY_BYTES_MAX, Body, Challenge, Error, First, Mode, OPEN_BYTES_MAX, Open, Place,
    Places, Read, Reader, Request, Response, Signed, place,
};

/// Checks one decoder of kind `kind` on `message`: a decoded message encodes to it,
/// and a refusal is the one that its first byte gives.
fn decode<T>(
    message: &[u8],
    kind: u8,
    decode: fn(&[u8]) -> Result<T, Error>,
    encode: fn(&T) -> Vec<u8>,
) -> Option<T> {
    match (message.first(), decode(message)) {
        (_, Ok(decoded)) => {
            assert_eq!(encode(&decoded), message, "the message changed");
            Some(decoded)
        }
        (None, Err(error)) => {
            panic!("{error:?} is not the refusal of an empty message")
        }
        (Some(&first), Err(error)) if first != kind => {
            assert_eq!(error, Error::Kind { kind: first }, "{message:?}");
            None
        }
        (Some(_), Err(error)) => {
            assert!(
                matches!(
                    error,
                    Error::Length { .. }
                        | Error::Subject(_)
                        | Error::SmallOrder
                        | Error::Oversize { .. }
                        | Error::Index { .. }
                ),
                "{error:?} is not a refusal of the bytes of {message:?}"
            );
            None
        }
    }
}

fn challenge(challenge: &Challenge) -> Vec<u8> {
    let mut out = vec![0; Challenge::LEN];
    challenge.encode(&mut out);
    out
}

fn signed(signed: &Signed) -> Vec<u8> {
    let mut out = vec![0; signed.encoded_len()];
    signed.encode(&mut out);
    out
}

fn request(request: &Request) -> Vec<u8> {
    let mut out = vec![0; Request::LEN];
    request.encode(&mut out);
    out
}

fn response(response: &Response) -> Vec<u8> {
    let mut out = vec![0; Response::LEN];
    response.encode(&mut out);
    out
}

fn read_message(read: &Read) -> Vec<u8> {
    let mut out = vec![0; Read::LEN];
    read.encode(&mut out);
    out
}

fn places(places: &Places) -> Vec<u8> {
    let mut out = vec![0; Places::LEN];
    places.encode(&mut out);
    out
}

/// `run` as one message of a run of places.
fn encode_run(run: &[Place]) -> Vec<u8> {
    let mut messages = place::messages(run, usize::MAX);
    let message = messages.next().expect("a run of places is a message");
    assert!(messages.next().is_none(), "a run of places is one message");
    let mut out = vec![0; message.encoded_len()];
    message.encode(&mut out);
    out
}

/// Checks the decoder of a message with no kind byte, an open: one that decodes must
/// encode to its bytes.
fn unkinded(message: &[u8]) {
    if let Ok(open) = Open::decode(message) {
        let mut out = vec![0; open.encoded_len()];
        open.encode(&mut out);
        assert_eq!(out, message, "the open changed");
    }
}

/// Drives a reader of `mode` on `bytes` until it gives the last place, then ends the
/// stream. A refused message leaves the reader as it was. Each message that it takes
/// decodes alone as places or as a run that encodes to it, a run holds no more places
/// than remain and a stamp at the index place, and only the message with the last
/// place gives the start: the places of the run, their key set in place order, and
/// the decoder of a session of the count and the mode. An end before the last place
/// gives `Error::Finished`.
fn drive(bytes: &[u8], mode: Mode) {
    let mut reader = Reader::new(mode);
    let mut run: Option<(Places, u32, Vec<Place>)> = None;
    for message in fuzz::messages(bytes) {
        let start = match reader.decode(Some(message)) {
            Ok(start) => start,
            Err(error) => {
                if let Some(repeated) = last(run.as_ref(), message) {
                    let expected = repeated.map(|key| Error::Repeated { key });
                    assert_eq!(Some(error), expected, "a last message refused");
                }
                continue;
            }
        };
        let Some((reply, remain, all)) = &mut run else {
            let places = Places::decode(message).expect("the places decode alone");
            let mut out = [0; Places::LEN];
            places.encode(&mut out);
            assert_eq!(out, message, "the places changed");
            assert!(start.is_none(), "the places gave the start");
            run = Some((places, places.count, Vec::new()));
            continue;
        };
        let places = place::decode(message).expect("the run decodes alone");
        assert_eq!(encode_run(&places), message, "the run changed");
        if let Some(index) = index_place(*reply, *remain, &places) {
            assert_eq!(index.data_type, Type::Scalar(Scalar::Stamp));
        }
        *remain -= u32::try_from(places.len()).expect("the run fits a u32");
        all.extend(places);
        let Some(start) = start else {
            assert_ne!(*remain, 0, "the last place gave no start");
            continue;
        };
        assert_eq!(*remain, 0, "a start before the last place");
        assert_eq!(start.places, *all, "the places differ");
        let keys = start.set.entries().iter().map(|entry| entry.key);
        assert!(
            keys.eq(all.iter().map(|place| place.key)),
            "the set differs"
        );
        let index = usize::try_from(reply.index).expect("a u32 fits a usize");
        assert_eq!(start.set.groups(), [index], "the index differs");
        let session = hub::Reader::new(&hub::Open {
            mode: match mode {
                Mode::Latest => hub::Mode::Latest,
                Mode::Complete => hub::Mode::Complete { limit_bytes: 0 },
            },
            channels: reply.count,
        });
        // `hub::Reader` has no `PartialEq`. Its derived `Debug` shows each field, so
        // an equal text is an equal state; `wire_hub_reader` checks its replies.
        assert_eq!(
            format!("{:?}", start.home),
            format!("{session:?}"),
            "the home differs"
        );
        return;
    }
    assert_eq!(reader.decode(None).err(), Some(Error::Finished));
}

/// The index place in `places`, a message of the run after `remain` places remained.
fn index_place(reply: Places, remain: u32, places: &[Place]) -> Option<&Place> {
    let at = usize::try_from(reply.index.wrapping_sub(reply.count - remain)).ok();
    at.and_then(|at| places.get(at))
}

/// When `message` is a valid last message of `run`, the least key that its places,
/// with those before it, name twice, if any.
fn last(
    run: Option<&(Places, u32, Vec<Place>)>,
    message: &[u8],
) -> Option<Option<channel::Key>> {
    let (reply, remain, all) = run?;
    let places = place::decode(message).ok()?;
    let stamp = Type::Scalar(Scalar::Stamp);
    if usize::try_from(*remain).ok()? != places.len()
        || index_place(*reply, *remain, &places).is_some_and(|at| at.data_type != stamp)
    {
        return None;
    }
    let mut keys: Vec<_> = all.iter().chain(&places).map(|place| place.key).collect();
    keys.sort_unstable();
    let repeated = keys.windows(2).find(|pair| pair[0] == pair[1]);
    Some(repeated.map(|pair| pair[0]))
}

/// Checks each decoder on `message`, and gives its body when it is a request, a
/// response, or a read.
fn fixed(message: &[u8]) -> Option<(Body, u64)> {
    unkinded(message);
    let first = match *message {
        [5, ..] => Request::decode(message).map(First::Request),
        [6, ..] => Read::decode(message).map(First::Read),
        [kind, ..] => Err(Error::Kind { kind }),
        [] => Err(Error::Empty),
    };
    assert_eq!(First::decode(message), first, "{message:?}");
    if message.is_empty() {
        assert_eq!(Challenge::decode(message), Err(Error::Empty));
        assert_eq!(Signed::decode(message), Err(Error::Empty));
        assert_eq!(Request::decode(message), Err(Error::Empty));
        assert_eq!(Response::decode(message), Err(Error::Empty));
        assert_eq!(Read::decode(message), Err(Error::Empty));
        assert_eq!(Places::decode(message), Err(Error::Empty));
        return None;
    }
    decode(message, 4, Challenge::decode, challenge);
    decode(message, 4, Signed::decode, signed);
    decode(message, 6, Places::decode, places);
    let sent = decode(message, 5, Request::decode, request);
    let answered = decode(message, 5, Response::decode, response);
    let opened = decode(message, 6, Read::decode, read_message);
    let (body, length, cap) = match (sent, answered, opened) {
        (Some(sent), None, None) => (sent.body(), sent.length, BODY_BYTES_MAX),
        (None, Some(answered), None) => {
            (answered.body(), answered.length, BODY_BYTES_MAX)
        }
        (None, None, Some(opened)) => (opened.body(), opened.length, OPEN_BYTES_MAX),
        (None, None, None) => return None,
        _ => {
            panic!("{message:?} is more than one of a request, a response, and a read")
        }
    };
    assert!(length <= cap, "a body over the cap came");
    Some((body, length))
}

/// Each decoder must read each message, and each message after the first must be
/// taken or refused as the rest of the body of the first gives.
fn read(bytes: &[u8]) {
    drive(bytes, Mode::Latest);
    drive(bytes, Mode::Complete);
    let mut messages = fuzz::messages(bytes);
    let Some((mut body, length)) = messages.next().and_then(fixed) else {
        for message in messages {
            fixed(message);
        }
        return;
    };
    let mut remain = usize::try_from(length).expect("the body fits");
    assert_eq!(body.remain(), remain, "the body has another length");
    for message in messages {
        fixed(message);
        let len = message.len();
        let expected = if remain == 0 {
            Err(Error::Trailing)
        } else if len == 0 {
            Err(Error::Empty)
        } else if len > remain {
            Err(Error::Body { len, remain })
        } else {
            remain -= len;
            Ok(message)
        };
        assert_eq!(body.take(message), expected, "{message:?}");
        assert_eq!(body.remain(), remain, "the rest of the body changed");
    }
    let ended = match remain {
        0 => Ok(()),
        remain => Err(Error::Unfinished { remain }),
    };
    assert_eq!(body.end(), ended, "the body ended at another length");
}

/// Each valid message made from `input` must decode to itself. A key of small order
/// or a subject that is not a name is not valid, so it becomes a fixed one.
fn write(input: &mut Unstructured) -> arbitrary::Result<()> {
    let key = PublicKey::new(input.arbitrary()?)
        .unwrap_or_else(|_small| PublicKey::new([9; 32]).expect("a valid key"));
    let subject = input
        .arbitrary::<&str>()?
        .parse()
        .unwrap_or_else(|_bad| "ops.ana".parse().expect("a name"));
    let signed = Signed {
        hello: Hello {
            subject,
            key,
            via: node::Key::from_u128(input.arbitrary()?),
            connection: connection::Key(input.arbitrary()?),
            nonce: input.arbitrary()?,
            expires: Stamp::from_nanos(input.arbitrary()?),
        },
        signature: input.arbitrary()?,
    };
    let mut out = vec![0; signed.encoded_len()];
    signed.encode(&mut out);
    assert_eq!(Signed::decode(&out), Ok(signed), "the hello changed");

    let request = Request {
        length: input.int_in_range(0..=BODY_BYTES_MAX)?,
        signature: input.arbitrary()?,
    };
    let mut out = [0; Request::LEN];
    request.encode(&mut out);
    assert_eq!(Request::decode(&out), Ok(request), "the request changed");

    let challenge = Challenge {
        nonce: input.arbitrary()?,
        now: Interval {
            earliest: Stamp::from_nanos(input.arbitrary()?),
            latest: Stamp::from_nanos(input.arbitrary()?),
        },
    };
    let mut out = [0; Challenge::LEN];
    challenge.encode(&mut out);
    assert_eq!(
        Challenge::decode(&out),
        Ok(challenge),
        "the challenge changed"
    );

    let response = Response {
        length: input.int_in_range(0..=BODY_BYTES_MAX)?,
    };
    let mut out = [0; Response::LEN];
    response.encode(&mut out);
    assert_eq!(Response::decode(&out), Ok(response), "the response changed");
    write_read(input)
}

/// Each valid message of a read stream made from `input` must decode to itself.
fn write_read(input: &mut Unstructured) -> arbitrary::Result<()> {
    let read = Read {
        length: input.int_in_range(0..=OPEN_BYTES_MAX)?,
        signature: input.arbitrary()?,
    };
    let mut out = [0; Read::LEN];
    read.encode(&mut out);
    assert_eq!(
        First::decode(&out),
        Ok(First::Read(read)),
        "the read changed"
    );

    let count = input.int_in_range(1..=u32::MAX)?;
    let index = input.int_in_range(0..=count - 1)?;
    let reply = Places { count, index };
    let mut out = [0; Places::LEN];
    reply.encode(&mut out);
    assert_eq!(Places::decode(&out), Ok(reply), "the places changed");

    let patterns: Vec<&str> = input.arbitrary()?;
    if let Ok(select) = Selector::new(patterns) {
        let mode = if input.arbitrary()? {
            Mode::Complete
        } else {
            Mode::Latest
        };
        let open = Open { mode, select };
        let mut out = vec![0; open.encoded_len()];
        open.encode(&mut out);
        assert_eq!(Open::decode(&out), Ok(open), "the open changed");
    }

    let data_type = input.arbitrary::<&str>()?.parse();
    let name = input.arbitrary::<&str>()?.parse();
    if let (Ok(data_type), Ok(name)) = (data_type, name) {
        let run = [Place {
            key: channel::Key::from_u128(input.arbitrary()?),
            data_type,
            name,
        }];
        assert_eq!(
            place::decode(&encode_run(&run)),
            Ok(run.to_vec()),
            "the place changed"
        );
    }
    Ok(())
}

fuzz_target!(|bytes: &[u8]| {
    read(bytes);
    write(&mut Unstructured::new(bytes)).expect("an input that ends gives zeros");
});
