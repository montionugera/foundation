use std::ffi::CString;
use std::future;
use std::net::SocketAddr;

use types::time::{Span, Stamp};

use super::{Client, PORT, VALUE, fits, fitting, long, run, serve_on};
use crate::client::{Credentials, Large, Mode};
use crate::event;
use crate::ffi::{self, Status};
use crate::secure::{
    ANONYMOUS, BROKEN_TRUST, CLIENT, CLIENT_PEM, EC, EMPTY_URI, END, Identity, OTHER,
    Policy, SERVER, START, TRUST, TWO_URIS, UNNAMED,
};
use crate::server::Limits;
use crate::value::{Fault, Value};

/// The test server, which trusts [`CLIENT`].
const SERVED: Credentials<'_> = with(SERVER, CLIENT.certificate);

/// The test client, which trusts [`SERVER`].
const CLIENT_SIDE: Credentials<'_> = with(CLIENT, SERVER.certificate);

const MODES: [Mode; 2] = [Mode::Sign, Mode::SignAndEncrypt];

/// The credentials of `own` that trust `trust`.
const fn with(own: Identity, trust: &'static [u8]) -> Credentials<'static> {
    Credentials {
        certificate: own.certificate,
        key: own.key,
        trust,
    }
}

/// What a read of [`VALUE`] through a secure channel gives.
#[derive(Debug, PartialEq)]
struct Read {
    /// The value of [`VALUE`] on the server.
    value: Value,
    /// The answer of the read.
    answer: Result<Value, Fault>,
    /// The policy URI and the mode of the channel.
    channel: (String, ffi::Mode),
}

/// The policy URI and the mode of the channel of `client`, read from C: no call of
/// `Client` and no server log gives them, and a read alone also passes over None,
/// which the server offers too.
fn channel(client: &Client) -> (String, ffi::Mode) {
    let mut mode = ffi::Mode(0);
    // SAFETY: the client lives, and `mode` is a place to write.
    let policy =
        unsafe { ffi::test::shim_client_channel(client.raw.as_ptr(), &raw mut mode) };
    // SAFETY: open62541 gives a live string of the config of the client, or null.
    let policy = unsafe { policy.as_ref() }.expect("the client has a channel");
    // SAFETY: as above.
    (unsafe { policy.lossy() }, mode)
}

/// The connect of a client whose loop starts at `client_wall` with `client`, and its
/// read of [`VALUE`], to a server whose loop starts at `server_wall` with `server`,
/// which offers `policies` in that order, in `mode`, in a run of `seed`, and the
/// digest of the run.
fn connect(
    seed: u64,
    (server, server_wall): (Credentials<'static>, Stamp),
    (client, client_wall): (Credentials<'static>, Stamp),
    policies: &[Policy],
    mode: Mode,
) -> (Result<Read, Status>, u64) {
    let policies = policies.to_vec();
    run(seed, async move |node| {
        let time = event::Time::since(node.clock(), server_wall);
        serve_on(
            &node,
            4,
            time,
            Some((&server, &policies)),
            async |mut peer| {
                let mut connected = peer.secure(mode, &client, client_wall).await?;
                let read = Read {
                    value: peer.value,
                    answer: connected.read_one(&VALUE).await,
                    channel: channel(&connected),
                };
                connected.close().await;
                Ok(read)
            },
        )
        .await
    })
}

/// A read from a server that offers `policies`, in `mode` at [`START`], in a run of
/// `seed`.
fn read(seed: u64, policies: &[Policy], mode: Mode) -> (Result<Read, Status>, u64) {
    connect(seed, (SERVED, START), (CLIENT_SIDE, START), policies, mode)
}

#[test]
fn a_client_reads_with_each_policy_in_each_mode() {
    for policy in Policy::ALL {
        for mode in MODES {
            let (read, _) = read(0, &[policy], mode);
            let read = read.expect("the client connects");
            assert_eq!(read.answer, Ok(read.value), "{policy:?} in {mode:?}");
            assert_eq!(read.channel, (uri(policy), mode.into()));
        }
    }
}

/// The URI of `policy`.
fn uri(policy: Policy) -> String {
    policy.uri().to_str().expect("the URI is UTF-8").to_owned()
}

#[test]
fn a_client_takes_the_policy_of_the_highest_level() {
    for policies in [
        Policy::ALL.as_slice(),
        &[Policy::Aes256Sha256RsaPss, Policy::Basic256Sha256],
    ] {
        for mode in MODES {
            let (read, _) = read(0, policies, mode);
            let read = read.expect("the client connects");
            assert_eq!(read.answer, Ok(read.value), "{policies:?} in {mode:?}");
            let channel = (uri(Policy::Aes256Sha256RsaPss), mode.into());
            assert_eq!(read.channel, channel, "{policies:?}");
        }
    }
}

/// The connect of a client with [`CLIENT`] that trusts `trust`, in `Sign`.
fn trusting(trust: &'static [u8]) -> Result<Read, Status> {
    let client = with(CLIENT, trust);
    let (read, _) = connect(
        0,
        (SERVED, START),
        (client, START),
        &Policy::ALL,
        Mode::Sign,
    );
    read
}

#[test]
fn a_client_that_trusts_a_pem_of_two_certificates_reads() {
    let read = trusting(TRUST).expect("the client connects");
    assert_eq!(read.answer, Ok(read.value));
}

#[test]
fn a_client_with_a_pem_certificate_and_key_reads() {
    let client = with(CLIENT_PEM, SERVER.certificate);
    let (read, _) = connect(
        0,
        (SERVED, START),
        (client, START),
        &Policy::ALL,
        Mode::Sign,
    );
    let read = read.expect("the client connects");
    assert_eq!(read.answer, Ok(read.value));
}

#[test]
fn a_trust_whose_second_certificate_does_not_parse_ends_the_connect() {
    assert_eq!(trusting(BROKEN_TRUST), Err(Status::BAD_INTERNAL_ERROR));
}

#[test]
fn a_trust_that_does_not_parse_ends_the_connect() {
    assert_eq!(trusting(b"no certificate"), Err(Status::BAD_INTERNAL_ERROR));
}

#[test]
fn an_empty_trust_ends_the_connect() {
    assert_eq!(trusting(b""), Err(Status::BAD_INTERNAL_ERROR));
}

/// The connect of a client with `own` that trusts [`SERVER`], in `Sign`, to a port
/// where no server listens, so that a refusal before the dial is the only error that
/// is not one of the dial.
fn refused(own: Identity) -> Result<(), Status> {
    let (connect, _) = run(0, async move |node| {
        let client = with(own, SERVER.certificate);
        let mut rng = node.entropy().rng();
        let url = format!("opc.tcp://{}", SocketAddr::new(node.addresses()[0], PORT));
        let url = CString::new(url).expect("the URL holds no NUL");
        let connect = Client::connect(
            event::Time::since(node.clock(), START),
            tracing::Span::none(),
            node.net(),
            &mut rng,
            &url,
            Some((&client, Mode::Sign)),
            future::pending(),
        );
        connect.await.map(|_| ())
    });
    connect
}

#[test]
fn a_certificate_that_does_not_parse_ends_the_connect_before_the_dial() {
    let own = Identity {
        certificate: b"no certificate",
        ..CLIENT
    };
    assert_eq!(refused(own), Err(Status::BAD_CERTIFICATE_INVALID));
}

#[test]
fn a_certificate_with_no_rsa_key_ends_the_connect_before_the_dial() {
    assert_eq!(refused(EC), Err(Status::BAD_CERTIFICATE_INVALID));
}

#[test]
fn a_key_that_does_not_parse_ends_the_connect_before_the_dial() {
    let own = Identity {
        key: b"no key",
        ..CLIENT
    };
    assert_eq!(refused(own), Err(Status::BAD_INVALID_ARGUMENT));
}

#[test]
fn a_key_of_another_certificate_ends_the_connect_before_the_dial() {
    let own = Identity {
        key: OTHER.key,
        ..CLIENT
    };
    assert_eq!(refused(own), Err(Status::BAD_SECURITY_CHECKS_FAILED));
}

#[test]
fn a_certificate_with_no_application_uri_ends_the_connect_before_the_dial() {
    assert_eq!(refused(ANONYMOUS), Err(Status::BAD_CERTIFICATE_URI_INVALID));
}

#[test]
fn a_certificate_with_no_alternative_name_ends_the_connect_before_the_dial() {
    assert_eq!(refused(UNNAMED), Err(Status::BAD_CERTIFICATE_URI_INVALID));
}

#[test]
fn a_certificate_whose_first_uri_is_empty_ends_the_connect_before_the_dial() {
    assert_eq!(refused(EMPTY_URI), Err(Status::BAD_CERTIFICATE_URI_INVALID));
}

#[test]
fn a_client_sends_the_first_uri_of_its_certificate() {
    let server = with(SERVER, TWO_URIS.certificate);
    let client = with(TWO_URIS, SERVER.certificate);
    let (uri, _) = run(0, async move |node| {
        let time = event::Time::since(node.clock(), START);
        serve_on(
            &node,
            4,
            time,
            Some((&server, Policy::ALL.as_slice())),
            async |mut peer| {
                let connected = peer.secure(Mode::Sign, &client, START).await;
                let connected = connected.expect("the client connects");
                // SAFETY: the client lives.
                let uri = unsafe { ffi::test::shim_client_uri(connected.raw.as_ptr()) };
                // SAFETY: open62541 gives a live string of the config of the client.
                let uri = unsafe { uri.as_ref() }.expect("the config holds a URI");
                // SAFETY: as above.
                let uri = unsafe { uri.lossy() };
                connected.close().await;
                uri
            },
        )
        .await
    });
    assert_eq!(uri, "urn:foundation:test:client");
}

#[test]
fn a_client_with_a_valid_certificate_dials() {
    assert_eq!(refused(CLIENT), Err(Status::BAD_CONNECTION_REJECTED));
}

#[test]
fn a_read_in_sign_and_encrypt_replays() {
    let spike = |seed| read(seed, &[Policy::Aes256Sha256RsaPss], Mode::SignAndEncrypt);
    let (first, digest) = spike(7);
    let first = first.expect("the client connects");
    let value = first.value;
    assert_eq!(first.answer, Ok(value));
    assert_eq!(spike(7), (Ok(first), digest));
    let (other, _) = spike(8);
    let other = other.expect("the client connects");
    assert_eq!(other.answer, Ok(other.value));
    assert_ne!(other.value, value);
}

#[test]
fn a_server_certificate_that_the_client_does_not_trust_ends_the_connect() {
    let other = with(OTHER, CLIENT.certificate);
    let (read, _) = connect(
        0,
        (other, START),
        (CLIENT_SIDE, START),
        &Policy::ALL,
        Mode::Sign,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_UNTRUSTED));
}

#[test]
fn a_client_past_the_end_of_the_server_certificate_ends_the_connect() {
    let (read, _) = connect(
        0,
        (SERVED, START),
        (CLIENT_SIDE, END + Span::SECOND),
        &Policy::ALL,
        Mode::Sign,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_TIME_INVALID));
}

#[test]
fn a_client_one_second_before_the_end_of_the_server_certificate_reads() {
    let (read, _) = connect(
        0,
        (SERVED, START),
        (CLIENT_SIDE, END - Span::SECOND),
        &Policy::ALL,
        Mode::Sign,
    );
    let read = read.expect("the client connects");
    assert_eq!(read.answer, Ok(read.value));
}

#[test]
fn a_client_one_second_before_the_start_of_the_server_certificate_ends_the_connect() {
    let (read, _) = connect(
        0,
        (SERVED, START),
        (CLIENT_SIDE, START - Span::SECOND),
        &Policy::ALL,
        Mode::Sign,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_TIME_INVALID));
}

#[test]
fn a_server_past_the_end_of_the_client_certificate_refuses_the_channel() {
    let (read, _) = connect(
        0,
        (SERVED, END + Span::SECOND),
        (CLIENT_SIDE, START),
        &Policy::ALL,
        Mode::Sign,
    );
    // The server sends the status of its check in the ERR message.
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_TIME_INVALID));
}

/// What a read of the largest request after [`VALUE`] in one message gives.
#[derive(Debug)]
struct Largest {
    /// The answer of the read.
    read: Result<Vec<Result<Value, Fault>>, Status>,
    /// The [`Large`] of a request whose last node is 1 byte longer.
    past: Vec<Large>,
    /// The value of [`VALUE`] on the server.
    value: Value,
}

/// The read of the largest request after [`VALUE`] that a client with `policy` in
/// `mode` puts in one message, to a server with a buffer of `buffer` bytes (0 for the
/// default).
fn largest_request(policy: Policy, mode: Mode, buffer: u32) -> Largest {
    let (reads, _) = run(0, async move |node| {
        let time = event::Time::since(node.clock(), START);
        serve_on(
            &node,
            4,
            time,
            Some((&SERVED, [policy].as_slice())),
            async |mut peer| {
                peer.server.set_limits(Limits {
                    buffer,
                    ..Limits::default()
                });
                let mut client = peer
                    .secure(mode, &CLIENT_SIDE, START)
                    .await
                    .expect("the client connects");
                let length = fitting(&client, &[], 1 << 20);
                peer.server.add_scalar(&long(length), Value::F64(2.5));
                let fit = fits(client.nodes(&[VALUE, long(length)]));
                let read = client.read(&fit).await.map(<[_]>::to_vec);
                let past = client.nodes(&[VALUE, long(length + 1)]).large().to_vec();
                client.close().await;
                Largest {
                    read,
                    past,
                    value: peer.value,
                }
            },
        )
        .await
    });
    reads
}

#[test]
fn a_request_that_fits_by_the_client_reads_in_sign() {
    let Largest { read, value, .. } =
        largest_request(Policy::Basic256Sha256, Mode::Sign, 0);
    assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]));
}

#[test]
fn a_request_that_fits_by_the_client_reads_in_sign_and_encrypt() {
    let Largest { read, value, .. } =
        largest_request(Policy::Aes256Sha256RsaPss, Mode::SignAndEncrypt, 0);
    assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]));
}

/// A chunk of 65536 bytes holds 24 bytes of headers and a signature of 32 bytes, and in
/// `SignAndEncrypt` 1 byte of padding, for each policy.
#[test]
fn a_request_takes_the_chunks_of_one_message_with_each_policy_in_each_mode() {
    for policy in Policy::ALL {
        for (mode, payload) in [(Mode::Sign, 65_480), (Mode::SignAndEncrypt, 65_479)] {
            let Largest { read, past, value } = largest_request(policy, mode, 0);
            let case = format!("{policy:?} in {mode:?}");
            assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]), "{case}");
            let most = 8 * payload - 7 * 7;
            let large = Large {
                index: 1,
                bytes: most + 1,
                most,
            };
            assert_eq!(past, [large], "{case}");
        }
    }
}

/// A chunk of 8200 bytes, 8 past a block of 16 bytes, also leaves those 8 bytes empty.
#[test]
fn a_chunk_leaves_its_bytes_past_a_block_empty() {
    let Largest { read, past, value } =
        largest_request(Policy::Basic256Sha256, Mode::Sign, 8200);
    assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]));
    let most = 8 * (8200 - 24 - 32 - 8) - 7 * 7;
    let large = Large {
        index: 1,
        bytes: most + 1,
        most,
    };
    assert_eq!(past, [large]);
}

/// A server with a buffer of 8192 bytes and 4 chunks sends an answer of at most 4
/// chunks of 8136 bytes less 21 in `Sign`, 955 Doubles of 34 bytes after 36, and of
/// 8120 in `SignAndEncrypt`, 953 Doubles.
#[test]
fn an_answer_takes_the_chunks_of_the_server_in_each_mode() {
    for (mode, split) in [(Mode::Sign, [955, 1]), (Mode::SignAndEncrypt, [953, 3])] {
        let (reads, _) = run(0, async move |node| {
            let time = event::Time::since(node.clock(), START);
            serve_on(
                &node,
                4,
                time,
                Some((&SERVED, [Policy::Basic256Sha256].as_slice())),
                async |mut peer| {
                    peer.server.set_limits(Limits {
                        buffer: 8192,
                        chunks: 4,
                        ..Limits::default()
                    });
                    let mut client = peer
                        .secure(mode, &CLIENT_SIDE, START)
                        .await
                        .expect("the client connects");
                    let nodes = fits(client.nodes(&vec![VALUE; 956]));
                    let values = client.read(&nodes).await.map(<[_]>::to_vec);
                    client.close().await;
                    (nodes.counts(), values, peer.value)
                },
            )
            .await
        });
        let (counts, values, value) = reads;
        let read = (counts, values);
        assert_eq!(read, (split.to_vec(), Ok(vec![Ok(value); 956])), "{mode:?}");
    }
}

/// In `SignAndEncrypt`, open62541 gives a chunk of 8135 bytes from a buffer of 8193, 1
/// less than Part 6, and one of 8199 from 8264, 1 less than Part 6 too. An answer of 4
/// chunks then holds 955 and 962 Doubles.
#[test]
fn an_answer_takes_the_payload_of_open62541_when_it_is_the_smaller() {
    for (buffer, split) in [(8193, [955, 45]), (8264, [962, 38])] {
        let (reads, _) = run(0, async move |node| {
            let time = event::Time::since(node.clock(), START);
            serve_on(
                &node,
                4,
                time,
                Some((&SERVED, [Policy::Basic256Sha256].as_slice())),
                async |mut peer| {
                    peer.server.set_limits(Limits {
                        buffer,
                        chunks: 4,
                        ..Limits::default()
                    });
                    let mut client = peer
                        .secure(Mode::SignAndEncrypt, &CLIENT_SIDE, START)
                        .await
                        .expect("the client connects");
                    let nodes = fits(client.nodes(&vec![VALUE; 1000]));
                    let values = client.read(&nodes).await.map(<[_]>::to_vec);
                    client.close().await;
                    (nodes.counts(), values, peer.value)
                },
            )
            .await
        });
        let (counts, values, value) = reads;
        let read = (counts, values);
        assert_eq!(
            read,
            (split.to_vec(), Ok(vec![Ok(value); 1000])),
            "{buffer}"
        );
    }
}

/// A received chunk holds the smaller of the payload of a sent one and the
/// `MaxBodySize` of Part 6. In `SignAndEncrypt`, the second is 508 blocks of 16 bytes
/// less 8 in a chunk of 8192 bytes, and 509 blocks less 8 in one of 8200, 1 byte past
/// the first.
#[test]
fn a_received_chunk_holds_the_smaller_of_two_payloads() {
    for (mode, buffer, payloads) in [
        (Mode::Sign, 8192, [8136, 8136]),
        (Mode::SignAndEncrypt, 8192, [8135, 8120]),
        (Mode::Sign, 8200, [8136, 8136]),
        (Mode::SignAndEncrypt, 8200, [8135, 8135]),
    ] {
        let (limits, _) = run(0, async move |node| {
            let time = event::Time::since(node.clock(), START);
            serve_on(
                &node,
                4,
                time,
                Some((&SERVED, [Policy::Basic256Sha256].as_slice())),
                async |mut peer| {
                    peer.server.set_limits(Limits {
                        buffer,
                        ..Limits::default()
                    });
                    let client = peer
                        .secure(mode, &CLIENT_SIDE, START)
                        .await
                        .expect("the client connects");
                    let mut limits = [0; 5];
                    // SAFETY: the client lives, and its channel is open.
                    let status = Status(unsafe {
                        ffi::shim_client_limits(client.raw.as_ptr(), &raw mut limits)
                    });
                    client.close().await;
                    (status, [limits[0], limits[1]])
                },
            )
            .await
        });
        assert_eq!(limits, (Status::GOOD, payloads), "{mode:?} {buffer}");
    }
}
