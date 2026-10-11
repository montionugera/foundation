//! The decoders of `wire::hub::client` never panic, each but `Open::decode` refuses a
//! message of another kind with `Error::Kind`, each message that decodes encodes to
//! its bytes, each body ends at its length and nowhere else, each refusal of a body is
//! the one that its rest gives, a read stream gives its places and then refuses each
//! message that a session of their count refuses, with its error, and each valid
//! message made from the input reads back.
//!
//! Input: the messages of one stream (`fuzz::messages`). Each decoder reads each
//! message, an open among them; the body of the first message, when it is a request, a
//! response, or a read, takes each later message; and a `client::Reader` of each mode
//! takes each message until it refuses one.

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
use wire::hub::client::{
    BODY_BYTES_MAX, Body, Challenge, First, FromNode, Mode, OPEN_BYTES_MAX, Open,
    Place, Places, Read, Reader, Request, Response, Signed, place,
};
use wire::hub::{self, Error};

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

/// The places in `message`, the first message of a read stream.
fn first_places(message: &[u8]) -> Result<Places, Error> {
    match Reader::new(Mode::Latest).decode(message)? {
        FromNode::Places(places) => Ok(places),
        other => panic!("{other:?} is the first message of a read stream"),
    }
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

/// Drives a reader of `mode` on `bytes` until it refuses a message. Each places and
/// each run that it gives encodes to its message, a run holds no more places than
/// remain and a stamp at the index place, and the stream gives no body and may not end
/// before the last place. After it, the reader answers as a session of the count does.
fn drive(bytes: &[u8], mode: Mode) {
    let mut reader = Reader::new(mode);
    let mut run: Option<(Places, u32)> = None;
    let mut session: Option<hub::Reader> = None;
    for message in fuzz::messages(bytes) {
        let decoded = reader.decode(message);
        if let Some(session) = &mut session {
            assert_eq!(
                decoded.as_ref().err(),
                session.decode(message).as_ref().err(),
                "{message:?}"
            );
        } else {
            assert_eq!(reader.body(), None, "a body came before the last place");
            assert_eq!(
                reader.end(),
                Err(Error::Finished),
                "a read ended in its run"
            );
        }
        let Ok(decoded) = decoded else { return };
        match (decoded, &mut run) {
            (FromNode::Places(places), None) => {
                let mut out = [0; Places::LEN];
                places.encode(&mut out);
                assert_eq!(out, message, "the places changed");
                run = Some((places, places.count));
            }
            (FromNode::Run(places), Some((reply, remain))) => {
                assert_eq!(encode_run(&places), message, "the run changed");
                let first = reply.count - *remain;
                *remain -= u32::try_from(places.len()).expect("the run fits a u32");
                let at = usize::try_from(reply.index.wrapping_sub(first)).ok();
                if let Some(index) = at.and_then(|at| places.get(at)) {
                    assert_eq!(index.data_type, Type::Scalar(Scalar::Stamp));
                }
                if *remain == 0 {
                    session = Some(hub::Reader::new(&hub::Open {
                        mode: match mode {
                            Mode::Latest => hub::Mode::Latest,
                            Mode::Complete => hub::Mode::Complete { limit_bytes: 0 },
                        },
                        channels: reply.count,
                    }));
                }
            }
            (FromNode::Home(_), Some((_, 0))) => {}
            (other, _) => panic!("{other:?} came out of order"),
        }
    }
    if let Some(session) = &session {
        assert_eq!(reader.body(), session.body(), "the body differs");
        assert_eq!(reader.end(), session.end(), "the end differs");
    }
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
        assert_eq!(first_places(message), Err(Error::Empty));
        return None;
    }
    decode(message, 4, Challenge::decode, challenge);
    decode(message, 4, Signed::decode, signed);
    decode(message, 6, first_places, places);
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
    assert_eq!(first_places(&out), Ok(reply), "the places changed");

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
        let mut reader = Reader::new(Mode::Complete);
        let mut first = [0; Places::LEN];
        Places { count: 2, index: 1 }.encode(&mut first);
        reader.decode(&first).expect("the places decode");
        match reader.decode(&encode_run(&run)) {
            Ok(FromNode::Run(decoded)) => assert_eq!(decoded, run, "the place changed"),
            other => panic!("{other:?} is not the run"),
        }
    }
    Ok(())
}

fuzz_target!(|bytes: &[u8]| {
    read(bytes);
    write(&mut Unstructured::new(bytes)).expect("an input that ends gives zeros");
});
