use env::rng::Rng;
use sim::Sim;
use types::time::{Span, Stamp};

use super::{Client, VALUE, fits, fitting, long, run, serve_on};
use crate::child;
use crate::client::Large;
use crate::event::{self, Loop};
use crate::ffi::test::Mode;
use crate::ffi::{self, Status};
use crate::secure::{CLIENT, END, OTHER, Policy, SERVER, START, Security};
use crate::server::Limits;
use crate::value::{Fault, Value};

/// The test server, which trusts [`CLIENT`].
const SERVED: Security = Security {
    own: SERVER,
    trusted: CLIENT.certificate,
};

/// The test client, which trusts [`SERVER`].
const CLIENT_SIDE: Security = Security {
    own: CLIENT,
    trusted: SERVER.certificate,
};

const MODES: [Mode; 2] = [Mode::SIGN, Mode::SIGN_AND_ENCRYPT];

/// What a read of [`VALUE`] through a secure channel gives.
#[derive(Debug, PartialEq)]
struct Read {
    /// The value of [`VALUE`] on the server.
    value: Value,
    /// The answer of the read.
    answer: Result<Value, Fault>,
    /// The policy URI and the mode of the channel.
    channel: (String, Mode),
}

/// The URI of `policy`.
fn uri(policy: Policy) -> &'static str {
    match policy {
        Policy::Basic256Sha256 => {
            "http://opcfoundation.org/UA/SecurityPolicy#Basic256Sha256"
        }
        Policy::Aes128Sha256RsaOaep => {
            "http://opcfoundation.org/UA/SecurityPolicy#Aes128_Sha256_RsaOaep"
        }
        Policy::Aes256Sha256RsaPss => {
            "http://opcfoundation.org/UA/SecurityPolicy#Aes256_Sha256_RsaPss"
        }
    }
}

/// The policy URI and the mode of the channel of `client`, read from C: no call of
/// `Client` and no server log gives them, and a read alone also passes over None,
/// which the server offers too.
fn channel(client: &Client) -> (String, Mode) {
    let mut mode = Mode(0);
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
/// with `policy` in `mode`, in a run of `seed`, and the digest of the run.
fn connect(
    seed: u64,
    (server, server_wall): (Security, Stamp),
    (client, client_wall): (Security, Stamp),
    policy: Policy,
    mode: Mode,
) -> (Result<Read, Status>, u64) {
    run(seed, async move |node| {
        let time = event::Time::since(node.clock(), server_wall);
        serve_on(&node, 4, time, Some(&server), async |mut peer| {
            let mut connected = peer.secure(policy, mode, &client, client_wall).await?;
            let read = Read {
                value: peer.value,
                answer: connected.read_one(&VALUE).await,
                channel: channel(&connected),
            };
            connected.close().await;
            Ok(read)
        })
        .await
    })
}

/// A read with `policy` in `mode` at [`START`], in a run of `seed`.
fn read(seed: u64, policy: Policy, mode: Mode) -> (Result<Read, Status>, u64) {
    connect(seed, (SERVED, START), (CLIENT_SIDE, START), policy, mode)
}

#[test]
fn a_client_reads_with_each_policy_in_each_mode() {
    for policy in Policy::ALL {
        for mode in MODES {
            let (read, _) = read(0, policy, mode);
            let read = read.expect("the client connects");
            assert_eq!(read.answer, Ok(read.value), "{policy:?} in {mode:?}");
            assert_eq!(read.channel, (uri(policy).to_owned(), mode));
        }
    }
}

#[test]
fn a_read_in_sign_and_encrypt_replays() {
    let spike = |seed| read(seed, Policy::Aes256Sha256RsaPss, Mode::SIGN_AND_ENCRYPT);
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
    let other = Security {
        own: OTHER,
        ..SERVED
    };
    let (read, _) = connect(
        0,
        (other, START),
        (CLIENT_SIDE, START),
        Policy::Basic256Sha256,
        Mode::SIGN,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_UNTRUSTED));
}

#[test]
fn a_client_past_the_end_of_the_server_certificate_ends_the_connect() {
    let (read, _) = connect(
        0,
        (SERVED, START),
        (CLIENT_SIDE, END + Span::SECOND),
        Policy::Basic256Sha256,
        Mode::SIGN,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_TIME_INVALID));
}

#[test]
fn a_client_one_second_before_the_end_of_the_server_certificate_reads() {
    let (read, _) = connect(
        0,
        (SERVED, START),
        (CLIENT_SIDE, END - Span::SECOND),
        Policy::Basic256Sha256,
        Mode::SIGN,
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
        Policy::Basic256Sha256,
        Mode::SIGN,
    );
    assert_eq!(read, Err(Status::BAD_CERTIFICATE_TIME_INVALID));
}

#[test]
fn a_server_past_the_end_of_the_client_certificate_refuses_the_channel() {
    let (read, _) = connect(
        0,
        (SERVED, END + Span::SECOND),
        (CLIENT_SIDE, START),
        Policy::Basic256Sha256,
        Mode::SIGN,
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
        serve_on(&node, 4, time, Some(&SERVED), async |mut peer| {
            peer.server.set_limits(Limits {
                buffer,
                ..Limits::default()
            });
            let mut client = peer
                .secure(policy, mode, &CLIENT_SIDE, START)
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
        })
        .await
    });
    reads
}

#[test]
fn a_request_that_fits_by_the_client_reads_in_sign() {
    let Largest { read, value, .. } =
        largest_request(Policy::Basic256Sha256, Mode::SIGN, 0);
    assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]));
}

#[test]
fn a_request_that_fits_by_the_client_reads_in_sign_and_encrypt() {
    let Largest { read, value, .. } =
        largest_request(Policy::Aes256Sha256RsaPss, Mode::SIGN_AND_ENCRYPT, 0);
    assert_eq!(read, Ok(vec![Ok(value), Ok(Value::F64(2.5))]));
}

/// A chunk of 65536 bytes holds 24 bytes of headers and a signature of 32 bytes, and in
/// `SignAndEncrypt` 1 byte of padding, for each policy.
#[test]
fn a_request_takes_the_chunks_of_one_message_with_each_policy_in_each_mode() {
    for policy in Policy::ALL {
        for (mode, payload) in [(Mode::SIGN, 65_480), (Mode::SIGN_AND_ENCRYPT, 65_479)]
        {
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
        largest_request(Policy::Basic256Sha256, Mode::SIGN, 8200);
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
/// chunks of 8136 bytes less 21 in `Sign`, and of 8135 in `SignAndEncrypt`: 955
/// Doubles of 34 bytes after 36, and not 956.
#[test]
fn an_answer_takes_the_chunks_of_the_server_in_each_mode() {
    for mode in MODES {
        let (reads, _) = run(0, async move |node| {
            let time = event::Time::since(node.clock(), START);
            serve_on(&node, 4, time, Some(&SERVED), async |mut peer| {
                peer.server.set_limits(Limits {
                    buffer: 8192,
                    chunks: 4,
                    ..Limits::default()
                });
                let mut client = peer
                    .secure(Policy::Basic256Sha256, mode, &CLIENT_SIDE, START)
                    .await
                    .expect("the client connects");
                let nodes = fits(client.nodes(&vec![VALUE; 956]));
                let values = client.read(&nodes).await.map(<[_]>::to_vec);
                client.close().await;
                (nodes.counts(), values, peer.value)
            })
            .await
        });
        let (counts, values, value) = reads;
        let read = (counts, values);
        assert_eq!(read, (vec![955, 1], Ok(vec![Ok(value); 956])), "{mode:?}");
    }
}

/// Asks for the policy of index 3, past the last one, when `child::running()`.
#[test]
fn ask_past_the_last_policy() {
    if !child::running() {
        return;
    }
    let mut sim = Sim::new(sim::Config::default());
    let clock = sim.node(sim::node::Config::default()).clock();
    let events = Loop::new(
        event::Time::monotonic(clock),
        tracing::Span::none(),
        &mut Rng::from_seed(0),
    );
    // SAFETY: the loop outlives the client, which leaks when the process aborts.
    let client = unsafe { crate::client::unconnected(events.raw()) }.as_ptr();
    let security = CLIENT_SIDE.raw();
    // SAFETY: the client lives and has not connected, and it copies `security`.
    let status = unsafe {
        ffi::test::shim_client_secure(
            client,
            Mode::SIGN,
            Policy::ALL.len(),
            &raw const security,
        )
    };
    panic!("the client gave {:?}", Status(status))
}

#[test]
fn a_policy_index_past_the_last_aborts() {
    child::aborted_in_c("client::tests::secure::ask_past_the_last_policy", &[], "");
}
