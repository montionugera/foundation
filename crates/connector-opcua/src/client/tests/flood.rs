//! A device that sends MSG chunks with request ids that the client never sent, for
//! each of which open62541 logs a warning.

use std::ffi::CString;
use std::future::{self, poll_fn};
use std::io::IoSlice;
use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use env::clock::Clock;
use env::net::{Tcp, tcp};
use sim::{Sim, node};
use types::time::Span;

use super::{Client, PORT, options};
use crate::event;

const NONE: &[u8] = b"http://opcfoundation.org/UA/SecurityPolicy#None";

fn frame(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
    let length = u32::try_from(8 + body.len()).unwrap();
    let mut bytes = kind.to_vec();
    bytes.extend(length.to_le_bytes());
    bytes.extend(body);
    bytes
}

fn push_string(bytes: &mut Vec<u8>, text: Option<&[u8]>) {
    match text {
        None => bytes.extend((-1_i32).to_le_bytes()),
        Some(text) => {
            bytes.extend(u32::try_from(text.len()).unwrap().to_le_bytes());
            bytes.extend(text);
        }
    }
}

async fn read_exact(stream: &mut Tcp, mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let read = poll_fn(|cx| stream.poll_read(cx, bytes)).await.unwrap();
        assert!(read > 0, "the client closed the stream");
        bytes = &mut bytes[read..];
    }
}

async fn message(stream: &mut Tcp) -> Vec<u8> {
    let mut bytes = vec![0; 8];
    read_exact(stream, &mut bytes).await;
    let length = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    bytes.resize(usize::try_from(length).unwrap(), 0);
    read_exact(stream, &mut bytes[8..]).await;
    bytes
}

async fn write(stream: &mut Tcp, bytes: &[u8]) {
    let mut sent = 0;
    while sent < bytes.len() {
        let parts = [IoSlice::new(&bytes[sent..])];
        sent += poll_fn(|cx| stream.poll_write(cx, &parts)).await.unwrap();
    }
}

/// The MSG chunks of `count` request ids from `first` on, which the client never sent.
fn unknown(first: u32, count: u32) -> Vec<u8> {
    let mut chunks = Vec::new();
    for n in first..first + count {
        let mut msg = Vec::new();
        for field in [1_u32, 1, 2 + n, 0x4000_0000 + n, 0] {
            msg.extend(field.to_le_bytes());
        }
        chunks.extend(frame(*b"MSGF", &msg));
    }
    chunks
}

/// A device that answers the HEL and the OPN of a client, sends 2000 MSG chunks of
/// unknown request ids, and after 1.1 s one more.
async fn device(mut stream: Tcp, clock: Clock) {
    let hel = message(&mut stream).await;
    assert_eq!(&hel[..4], b"HELF");
    let mut ack = Vec::new();
    for field in [0_u32, 1 << 16, 1 << 16, 0, 0] {
        ack.extend(field.to_le_bytes());
    }
    write(&mut stream, &frame(*b"ACKF", &ack)).await;
    let opn = message(&mut stream).await;
    assert_eq!(&opn[..4], b"OPNF");
    let mut body = 1_u32.to_le_bytes().to_vec(); // channel id
    push_string(&mut body, Some(NONE));
    push_string(&mut body, None);
    push_string(&mut body, None);
    body.extend(1_u32.to_le_bytes()); // sequence number
    body.extend(1_u32.to_le_bytes()); // request id
    body.extend([0x01, 0, 0xC1, 0x01]); // OpenSecureChannelResponse
    body.extend(0_i64.to_le_bytes()); // timestamp
    body.extend(0_u32.to_le_bytes()); // request handle
    body.extend(0_u32.to_le_bytes()); // Good
    body.push(0); // no diagnostics
    body.extend((-1_i32).to_le_bytes()); // no string table
    body.extend([0, 0, 0]); // no additional header
    body.extend(0_u32.to_le_bytes()); // server protocol version
    body.extend(1_u32.to_le_bytes()); // token channel id
    body.extend(1_u32.to_le_bytes()); // token id
    body.extend(0_i64.to_le_bytes()); // created at
    body.extend(600_000_u32.to_le_bytes()); // revised lifetime
    push_string(&mut body, None); // nonce
    write(&mut stream, &frame(*b"OPNF", &body)).await;
    write(&mut stream, &unknown(0, 2000)).await;
    clock.sleep(Span::from_nanos(1_100_000_000)).await;
    write(&mut stream, &unknown(2000, 1)).await;
    future::pending::<()>().await;
}

/// The lines of target `open62541` of a connect for 1.5 s to the device.
fn flooded() -> Vec<String> {
    let (capture, log) = connector::testing::Capture::new("open62541");
    let mut sim = Sim::new(sim::Config::default());
    let host = sim.node(node::Config::default());
    let config = env::shards::Config {
        name: "run".into(),
        core: None,
    };
    let done = Arc::new(Mutex::new(false));
    let (slot, node) = (Arc::clone(&done), host.clone());
    let start = host
        .shards()
        .logged(log)
        .start(config, move |_| async move {
            let local = SocketAddr::new(node.addresses()[0], PORT);
            let listen = tcp::Listen {
                local,
                backlog: 4,
                options: options(),
            };
            let mut listener = node.net().listen(&listen).expect("the port is free");
            let span =
                tracing::error_span!("connector", connector = "press", kind = "opcua");
            let mut rng = node.entropy().rng();
            let url = CString::new(format!("opc.tcp://{local}")).unwrap();
            let until = node.clock().sleep(Span::from_nanos(1_500_000_000));
            let mut connect = pin!(Client::connect(
                event::Time::monotonic(node.clock()),
                span,
                node.net(),
                &mut rng,
                &url,
                until,
            ));
            let mut dev: Option<Pin<Box<dyn Future<Output = ()>>>> = None;
            let out = poll_fn(|cx| {
                if dev.is_none()
                    && let Poll::Ready(stream) = listener.poll_accept(cx)
                {
                    dev = Some(Box::pin(device(stream.unwrap(), node.clock())));
                }
                if let Some(d) = dev.as_mut() {
                    assert!(d.as_mut().poll(cx).is_pending());
                }
                connect.as_mut().poll(cx)
            })
            .await;
            drop(out);
            *slot.lock().expect("no panic under the lock") = true;
        });
    drop(start.expect("a shard with no core starts"));
    sim.run().expect("the run ends");
    assert!(
        *done.lock().expect("no panic under the lock"),
        "the connect ends"
    );
    capture.lines()
}

/// The line of the warning `text`.
fn warned(category: &str, text: &str) -> String {
    format!(r#"WARN {text} connector="press" kind="opcua" category="{category}""#)
}

/// The 2001 warnings of the flood come in one window of 1 s: the first 100 go out, and
/// the count of the rest goes out before the first line of the next window.
#[test]
fn a_device_that_floods_the_log_gets_100_warnings_a_second_and_a_count_of_the_rest() {
    let mut want = vec![warned(
        "securechannel",
        "TCP 1\t| SC 1\t| The \"CreatedAt\" timestamp of the received \
         ChannelSecurityToken does not match with the local system clock",
    )];
    for n in 0..99 {
        let id = 0x4000_0000 + n;
        want.push(warned(
            "client",
            &format!("Request with unknown RequestId {id}"),
        ));
    }
    want.push(r#"WARN dropped connector="press" kind="opcua" lines=1901"#.to_owned());
    let id = 0x4000_0000 + 2000;
    want.push(warned(
        "client",
        &format!("Request with unknown RequestId {id}"),
    ));
    want.push(warned(
        "client",
        "FindServers failed with error code BadSecureChannelClosed. Continue with the \
         EndpointURL opc.tcp://10.0.0.1:4840.",
    ));
    assert_eq!(flooded(), want);
}
