use std::future::poll_fn;
use std::net::{Ipv4Addr, Ipv6Addr};

use env::net::Net;
use env::net::udp::{Receiver, Sender};

use super::*;

const V4: Ipv4Addr = Ipv4Addr::LOCALHOST;

fn config(local: SocketAddr) -> udp::Config {
    udp::Config {
        local,
        send_buffer_bytes: 1 << 16,
        recv_buffer_bytes: 1 << 16,
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()
        .unwrap()
}

fn bind(local: SocketAddr) -> (Sender, Receiver) {
    Net::new(super::super::Net).udp(&config(local)).unwrap()
}

fn transmit(destination: SocketAddr, contents: &[u8]) -> Transmit<'_> {
    Transmit {
        destination,
        source: None,
        ecn: Some(Ecn::Ect1),
        contents,
        segment: None,
    }
}

async fn send(sender: &mut Sender, transmit: &Transmit<'_>) {
    poll_fn(|cx| sender.poll_send(cx, transmit)).await.unwrap();
}

async fn receive(receiver: &mut Receiver) -> (Vec<u8>, Meta) {
    let mut buffer = [0; 64];
    let mut meta = [Meta::default()];
    let count = poll_fn(|cx| {
        receiver.poll_recv(cx, &mut [IoSliceMut::new(&mut buffer)], &mut meta)
    })
    .await
    .unwrap();
    assert_eq!(count, 1);
    (buffer[..meta[0].len].to_vec(), meta[0])
}

#[test]
fn a_socket_on_any_reaches_ipv4_and_gives_each_address_as_ipv4() {
    runtime().block_on(async {
        let (mut any, mut any_receiver) = bind((Ipv6Addr::UNSPECIFIED, 0).into());
        let (mut v4, mut v4_receiver) = bind((V4, 0).into());
        send(&mut any, &transmit(v4.local(), b"ping")).await;
        let (contents, meta) = receive(&mut v4_receiver).await;
        let source = SocketAddr::from((V4, any.local().port()));
        let expected = Meta {
            source,
            destination: Some(V4.into()),
            ecn: Some(Ecn::Ect1),
            len: 4,
            stride: 4,
        };
        assert_eq!((contents.as_slice(), meta), (&b"ping"[..], expected));
        send(&mut v4, &transmit(source, b"pong")).await;
        let (contents, meta) = receive(&mut any_receiver).await;
        let expected = Meta {
            source: v4.local(),
            ..expected
        };
        assert_eq!((contents.as_slice(), meta), (&b"pong"[..], expected));
    });
}

#[test]
fn a_held_address_is_in_use() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (held, _receiver) = bind((V4, 0).into());
    let local = held.local();
    let error = Net::new(super::super::Net).udp(&config(local)).unwrap_err();
    assert_eq!(error, Error::AddressInUse { local });
}

#[test]
fn sends_one_datagram_per_send_and_receives_a_batch_of_the_gro_size() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (sender, receiver) = bind((V4, 0).into());
    assert_eq!(sender.batch_max(), NonZeroUsize::MIN);
    #[cfg(target_os = "linux")]
    assert_eq!(receiver.batch_max().get(), 64);
    #[cfg(not(target_os = "linux"))]
    drop(receiver);
}

#[test]
#[should_panic(expected = "the sdk sends from one sender")]
fn a_second_sender_panics_while_the_first_lives() {
    runtime().block_on(async {
        let (mut first, _receiver) = bind((V4, 0).into());
        let mut second = first.clone();
        let destination = first.local();
        send(&mut first, &transmit(destination, b"a")).await;
        send(&mut second, &transmit(destination, b"b")).await;
    });
}

#[test]
fn a_sender_sends_after_the_sender_before_it_drops() {
    runtime().block_on(async {
        let (mut first, mut receiver) = bind((V4, 0).into());
        let mut second = first.clone();
        let destination = first.local();
        send(&mut first, &transmit(destination, b"a")).await;
        drop(first);
        send(&mut second, &transmit(destination, b"b")).await;
        assert_eq!(receive(&mut receiver).await.0, b"a");
        assert_eq!(receive(&mut receiver).await.0, b"b");
    });
}

#[test]
fn maps_each_outcome_of_a_send() {
    let remote = SocketAddr::from((V4, 9));
    let failed = |error| sent(Err(error), remote);
    let kind = |kind| failed(io::Error::from(kind));
    let code = |code| failed(io::Error::from_raw_os_error(code));
    assert_eq!(sent(Ok(()), remote), Poll::Ready(Ok(())));
    assert_eq!(kind(io::ErrorKind::WouldBlock), Poll::Pending);
    assert_eq!(code(MSGSIZE), Poll::Ready(Ok(())));
    let unreachable = Poll::Ready(Err(Error::Unreachable { remote }));
    assert_eq!(kind(io::ErrorKind::NetworkUnreachable), unreachable);
    assert_eq!(kind(io::ErrorKind::HostUnreachable), unreachable);
    let io = Poll::Ready(Err(Error::Io { code: NOPROTOOPT }));
    assert_eq!(code(NOPROTOOPT), io);
}

#[test]
fn maps_each_outcome_of_a_receive() {
    let refused = io::Error::from_raw_os_error(111);
    assert_eq!(received(Ok(3)), Poll::Ready(Ok(3)));
    assert_eq!(
        received(Err(io::ErrorKind::WouldBlock.into())),
        Poll::Pending
    );
    assert_eq!(
        received(Err(refused)),
        Poll::Ready(Err(Error::Io { code: 111 }))
    );
}

#[test]
#[should_panic(expected = "a socket call failed with no OS code: invalid data")]
fn an_error_with_no_os_code_panics() {
    drop(from_io(&io::Error::from(io::ErrorKind::InvalidData)));
}

#[test]
fn each_ecn_codepoint_round_trips() {
    for ecn in [Ecn::Ect0, Ecn::Ect1, Ecn::Ce] {
        assert_eq!(from_codepoint(to_codepoint(ecn)), ecn);
    }
}

#[test]
fn an_ipv4_address_maps_only_on_an_ipv6_socket() {
    let v4 = SocketAddr::from((V4, 9));
    let mapped = SocketAddr::from((V4.to_ipv6_mapped(), 9));
    let transmit = Transmit {
        source: Some(V4.into()),
        ..transmit(v4, b"")
    };
    let any = SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0));
    assert_eq!(
        route(any, &transmit),
        (mapped, Some(V4.to_ipv6_mapped().into()))
    );
    assert_eq!(route((V4, 0).into(), &transmit), (v4, Some(V4.into())));
}
