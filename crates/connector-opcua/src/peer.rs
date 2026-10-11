//! A raw OPC UA client for tests: it writes and reads the bytes of each message
//! itself, with no open62541.

use std::future::poll_fn;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::num::NonZeroUsize;

use env::net::{Net, Tcp, tcp};

use crate::ffi::Status;

/// The options of each stream.
const OPTIONS: tcp::Options = tcp::Options {
    send_buffer_bytes: 1 << 16,
    recv_buffer_bytes: 1 << 16,
    unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("invariant: 2^14 is not 0"),
    delayed: false,
};

/// Gives the `HEL` message of a client that asks for `url`, with buffers of 64 KiB
/// and no limit on a message or its chunks.
pub(crate) fn hel(url: &str) -> Vec<u8> {
    let length = u32::try_from(32 + url.len()).expect("a short URL");
    let url_length = u32::try_from(url.len()).expect("a short URL");
    let mut bytes = b"HELF".to_vec();
    for field in [length, 0, 1 << 16, 1 << 16, 0, 0, url_length] {
        bytes.extend(field.to_le_bytes());
    }
    bytes.extend(url.as_bytes());
    bytes
}

/// The security policy of a channel with no security.
const NONE: &[u8] = b"http://opcfoundation.org/UA/SecurityPolicy#None";

/// Connects to the OPC UA server at `remote` on `net` with raw bytes, opens a channel
/// with no security, creates a session, and closes the stream. It gives the GUIDs of
/// the session id and of the authentication token, which the server draws from the
/// random numbers of open62541 on its thread. Raw bytes start no loop of open62541,
/// so the peer sets no random numbers. The session is not activated, so the server
/// removes it with the channel.
///
/// # Panics
///
/// If the connect fails, or if the server answers with other than an `ACK`, an
/// `OpenSecureChannelResponse`, and a `CreateSessionResponse` with `Good`.
pub(crate) async fn session(net: &Net, remote: SocketAddr) -> [[u8; 16]; 2] {
    let config = tcp::Config {
        remote,
        options: OPTIONS,
    };
    let stream = &mut net.connect(&config).await.expect("the connect works");
    write(stream, &hel(&format!("opc.tcp://{remote}"))).await;
    let ack = message(stream).await;
    assert_eq!(&ack[..4], b"ACKF", "{ack:?}");
    let mut opn = 0_u32.to_le_bytes().to_vec();
    push_string(&mut opn, Some(NONE));
    for field in [-1_i32, -1, 1, 1] {
        opn.extend(field.to_le_bytes());
    }
    opn.extend([0x01, 0, 0xBE, 0x01]); // OpenSecureChannelRequest
    request_header(&mut opn);
    for field in [0_u32, 0, 1, 0, 600_000] {
        opn.extend(field.to_le_bytes());
    }
    write(stream, &frame(*b"OPNF", &opn)).await;
    let opened = message(stream).await;
    let at = 12 + 4 + NONE.len() + 4 + 4 + 8;
    let opened = response(&opened, *b"OPNF", at, [0x01, 0, 0xC1, 0x01]);
    let mut create = opened[4..12].to_vec(); // channel id and token id
    for field in [2_u32, 2] {
        create.extend(field.to_le_bytes());
    }
    create.extend([0x01, 0, 0xCD, 0x01]); // CreateSessionRequest
    request_header(&mut create);
    // The application description: two null strings, a null name, a client, two
    // null strings, and no URL.
    push_string(&mut create, None);
    push_string(&mut create, None);
    create.push(0);
    create.extend(1_u32.to_le_bytes());
    for _ in 0..3 {
        push_string(&mut create, None);
    }
    push_string(&mut create, None);
    push_string(&mut create, Some(format!("opc.tcp://{remote}").as_bytes()));
    for _ in 0..3 {
        push_string(&mut create, None);
    }
    create.extend(60_000_f64.to_le_bytes());
    create.extend(0_u32.to_le_bytes());
    write(stream, &frame(*b"MSGF", &create)).await;
    let created = message(stream).await;
    let created = response(&created, *b"MSGF", 24, [0x01, 0, 0xD0, 0x01]);
    drop(poll_fn(|cx| stream.poll_close(cx)).await);
    let (session, token) = created.split_at(19);
    [guid(session), guid(token)]
}

/// Adds the length of `text` and its bytes to `bytes`, or -1 for a null string.
fn push_string(bytes: &mut Vec<u8>, text: Option<&[u8]>) {
    let length = text.map_or(-1, |text| i32::try_from(text.len()).expect("short"));
    bytes.extend(length.to_le_bytes());
    bytes.extend(text.unwrap_or_default());
}

/// Adds a request header with no token, no audit entry, and no timeout to `bytes`. The
/// server refuses a time of 0.
fn request_header(bytes: &mut Vec<u8>) {
    bytes.extend([0, 0]);
    bytes.extend(1_i64.to_le_bytes());
    for field in [1_i32, 0, -1, 0] {
        bytes.extend(field.to_le_bytes());
    }
    bytes.extend([0, 0, 0]);
}

/// Gives the message of `kind` with `body`, in one final chunk.
fn frame(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + body.len()).expect("a short message");
    let mut bytes = kind.to_vec();
    bytes.extend(length.to_le_bytes());
    bytes.extend(body);
    bytes
}

/// Reads one message from `stream`.
async fn message(stream: &mut Tcp) -> Vec<u8> {
    let mut bytes = vec![0; 8];
    read_exact(stream, &mut bytes).await;
    let length = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
    bytes.resize(usize::try_from(length).expect("a 32-bit length"), 0);
    read_exact(stream, &mut bytes[8..]).await;
    bytes
}

async fn read_exact(stream: &mut Tcp, mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let read = poll_fn(|cx| stream.poll_read(cx, bytes)).await;
        let read = read.expect("the read works");
        assert!(read > 0, "the server closed the stream");
        bytes = &mut bytes[read..];
    }
}

/// Gives the body of the response of type `kind` in `message`, after its response
/// header. The type is at `at`.
///
/// # Panics
///
/// If the chunk of `message` is not `chunk`, its type is not `kind`, or its service
/// result is not `Good`.
fn response(message: &[u8], chunk: [u8; 4], at: usize, kind: [u8; 4]) -> &[u8] {
    assert_eq!(message[..4], chunk, "{message:?}");
    assert_eq!(message[at..at + 4], kind, "{message:?}");
    let header = &message[at + 4..at + 28];
    let result = u32::from_le_bytes(header[12..16].try_into().expect("4 bytes"));
    assert_eq!(Status(result), Status::GOOD, "{message:?}");
    &message[at + 28..]
}

/// Gives the GUID of the node id of namespace 1 at the start of `id`.
fn guid(id: &[u8]) -> [u8; 16] {
    let (encoding, guid) = id.split_at(3);
    assert_eq!(encoding, [0x04, 1, 0], "a GUID of namespace 1: {id:?}");
    guid[..16].try_into().expect("16 bytes")
}

/// Writes all of `bytes` to `stream`.
pub(crate) async fn write(stream: &mut Tcp, bytes: &[u8]) {
    let mut sent = 0;
    while sent < bytes.len() {
        let parts = [IoSlice::new(&bytes[sent..])];
        sent += poll_fn(|cx| stream.poll_write(cx, &parts))
            .await
            .expect("the write works");
    }
}
