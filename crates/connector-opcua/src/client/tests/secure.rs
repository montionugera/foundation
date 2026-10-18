use types::time::{Span, Stamp};

use super::{Client, VALUE, run, serve_secure};
use crate::ffi::test::Mode;
use crate::ffi::{self, Status};
use crate::secure::{CLIENT, END, OTHER, Policy, SERVER, START, Security};
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

/// The policy URI and the mode of the channel of `client`.
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
        serve_secure(&node, &server, server_wall, async |mut peer| {
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
