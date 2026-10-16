use std::future::poll_fn;
use std::net::{Ipv4Addr, SocketAddrV6};

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
fn two_senders_that_wait_together_each_send() {
    runtime().block_on(async {
        let (mut first, mut receiver) = bind((V4, 0).into());
        let mut second = first.clone();
        let destination = first.local();
        let (a, b) = (transmit(destination, b"a"), transmit(destination, b"b"));
        let mut pending = [true; 2];
        poll_fn(|cx| {
            if pending[0] {
                pending[0] = first.poll_send(cx, &a).map(Result::unwrap).is_pending();
            }
            if pending[1] {
                pending[1] = second.poll_send(cx, &b).map(Result::unwrap).is_pending();
            }
            if pending == [false; 2] {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        let mut contents = [
            receive(&mut receiver).await.0,
            receive(&mut receiver).await.0,
        ];
        contents.sort();
        assert_eq!(contents, [b"a", b"b"]);
    });
}

/// No portable send fills the OS send buffer or finds no route, so this test calls
/// `sent` itself.
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

/// An unconnected socket gets no ICMP error, so no portable receive fails, and this
/// test calls `received` itself.
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

/// Each socket call gives an OS code, so no public call reaches this panic.
#[test]
#[should_panic(expected = "a socket call failed with no OS code: invalid data")]
fn an_error_with_no_os_code_panics() {
    drop(from_io(&io::Error::from(io::ErrorKind::InvalidData)));
}

/// The sizes show only through the socket, a private field.
#[test]
fn sets_the_buffer_sizes_and_takes_ipv4_on_an_ipv6_socket() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let config = udp::Config {
        local: any_v6(),
        send_buffer_bytes: 1 << 12,
        recv_buffer_bytes: 1 << 13,
    };
    let (udp, _receiver) = super::bind(&config).unwrap();
    let socket = socket2::SockRef::from(&udp.0.socket);
    let kept = |bytes: usize| {
        if cfg!(target_os = "linux") {
            bytes * 2
        } else {
            bytes
        }
    };
    assert_eq!(socket.send_buffer_size().unwrap(), kept(1 << 12));
    assert_eq!(socket.recv_buffer_size().unwrap(), kept(1 << 13));
    assert!(!socket.only_v6().unwrap());
}

#[test]
fn each_ecn_codepoint_arrives() {
    runtime().block_on(async {
        let (mut sender, mut receiver) = bind((V4, 0).into());
        let destination = sender.local();
        for ecn in [Some(Ecn::Ect0), Some(Ecn::Ect1), Some(Ecn::Ce), None] {
            let to = Transmit {
                ecn,
                ..transmit(destination, b"a")
            };
            send(&mut sender, &to).await;
            assert_eq!(receive(&mut receiver).await.1.ecn, ecn);
        }
    });
}

/// TEST-NET-1 has no host. The route out leaves loopback, whose MTU is larger.
#[test]
fn a_datagram_over_the_path_mtu_is_lost_not_an_error() {
    runtime().block_on(async {
        let (mut sender, _receiver) = bind((Ipv4Addr::UNSPECIFIED, 0).into());
        let remote = SocketAddr::from(([192, 0, 2, 1], 9));
        let contents = [0; 9_500];
        let to = transmit(remote, &contents);
        assert_eq!(poll_fn(|cx| sender.poll_send(cx, &to)).await, Ok(()));
    });
}

fn v4(port: u16) -> SocketAddr {
    SocketAddr::from((V4, port))
}

fn mapped(port: u16) -> SocketAddr {
    SocketAddr::from((V4.to_ipv6_mapped(), port))
}

fn v6(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv6Addr::LOCALHOST, port))
}

fn any_v6() -> SocketAddr {
    SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
}

/// The outcome of one send of `transmit` from a socket bound to `local`.
fn sent_from(local: SocketAddr, transmit: &Transmit<'_>) -> Result<(), Error> {
    runtime().block_on(async {
        let (mut sender, _receiver) = bind(local);
        poll_fn(|cx| sender.poll_send(cx, transmit)).await
    })
}

#[test]
fn a_socket_on_an_address_reaches_only_its_family() {
    for (local, remote) in [
        (v4(0), mapped(9)),
        (v4(0), v6(9)),
        (v6(0), mapped(9)),
        (v6(0), v4(9)),
    ] {
        let unreachable = Err(Error::Unreachable { remote });
        assert_eq!(sent_from(local, &transmit(remote, b"a")), unreachable);
    }
}

#[test]
fn a_source_that_the_socket_cannot_send_from_is_invalid() {
    let invalid = Err(Error::Io { code: INVAL });
    for (local, source, remote) in [
        (any_v6(), Ipv6Addr::LOCALHOST.into(), v4(9)),
        (any_v6(), Ipv6Addr::LOCALHOST.into(), mapped(9)),
        (any_v6(), Ipv6Addr::UNSPECIFIED.into(), v6(9)),
        (v4(0), Ipv6Addr::LOCALHOST.into(), v4(9)),
    ] {
        let from = Transmit {
            source: Some(source),
            ..transmit(remote, b"a")
        };
        assert_eq!(
            sent_from(local, &from),
            invalid,
            "{local} {source} {remote}"
        );
    }
}

#[test]
fn a_socket_sends_from_an_ipv4_source_on_either_family() {
    runtime().block_on(async {
        let (held, mut receiver) = bind(v4(0));
        let destination = held.local();
        for local in [any_v6(), v4(0)] {
            let (mut sender, _receiver) = bind(local);
            for source in [IpAddr::V4(V4), IpAddr::V6(V4.to_ipv6_mapped())] {
                if local.is_ipv4() && source.is_ipv6() {
                    continue;
                }
                let from = Transmit {
                    source: Some(source),
                    ..transmit(destination, b"a")
                };
                send(&mut sender, &from).await;
                let (_, meta) = receive(&mut receiver).await;
                assert_eq!(meta.source.ip(), IpAddr::V4(V4), "{local} {source}");
            }
        }
    });
}

/// Linux takes an IPv4 address on an IPv6 socket, and refuses an IPv6 source to IPv4
/// in the kernel, so only macOS shows these rules through a send.
#[test]
fn routes_ipv4_as_mapped_only_on_an_ipv6_socket() {
    let from = |source: IpAddr| Transmit {
        source: Some(source),
        ..transmit(v4(2), b"")
    };
    let mapped_v4 = IpAddr::V6(V4.to_ipv6_mapped());
    let invalid = Err(Error::Io { code: INVAL });
    assert_eq!(
        route(any_v6(), &transmit(v4(2), b"")),
        Ok((mapped(2), None))
    );
    assert_eq!(
        route(any_v6(), &from(V4.into())),
        Ok((mapped(2), Some(mapped_v4)))
    );
    assert_eq!(route(any_v6(), &from(Ipv6Addr::LOCALHOST.into())), invalid);
    assert_eq!(route(v4(0), &from(V4.into())), Ok((v4(2), Some(V4.into()))));
}

/// Loopback has no link-local address, so these tests call `route` and `canonical`
/// themselves.
#[test]
fn keeps_the_scope_of_an_ipv6_address() {
    let link_local =
        SocketAddr::from(SocketAddrV6::new("fe80::1".parse().unwrap(), 9, 3, 7));
    assert_eq!(
        route(any_v6(), &transmit(link_local, b"")),
        Ok((link_local, None))
    );
    assert_eq!(canonical(link_local), link_local);
}

/// Polls `receiver` once, with a waker that does nothing.
fn poll_once(receiver: &mut Receiver) {
    let mut buffer = [0; 8];
    let mut meta = [Meta::default()];
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let buffers = &mut [IoSliceMut::new(&mut buffer)];
    drop(receiver.poll_recv(&mut cx, buffers, &mut meta));
}

#[test]
#[should_panic(expected = "a UDP receiver polls only on the thread of its first poll")]
fn a_receiver_that_polls_on_a_second_thread_panics() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (_sender, mut receiver) = bind(v4(0));
    poll_once(&mut receiver);
    #[expect(clippy::disallowed_methods, reason = "a poll on a second thread")]
    let second = std::thread::spawn(move || poll_once(&mut receiver));
    if let Err(payload) = second.join() {
        std::panic::resume_unwind(payload);
    }
}

#[test]
#[should_panic(expected = "a UDP sender polls only on the thread of its first poll")]
fn a_sender_that_polls_on_a_second_thread_panics() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let (mut sender, _receiver) = bind(v4(0));
    let to = sender.local();
    let mut cx = Context::from_waker(std::task::Waker::noop());
    drop(sender.poll_send(&mut cx, &transmit(to, b"a")));
    #[expect(clippy::disallowed_methods, reason = "a poll on a second thread")]
    let second = std::thread::spawn(move || {
        let mut cx = Context::from_waker(std::task::Waker::noop());
        drop(sender.poll_send(&mut cx, &transmit(to, b"b")));
    });
    if let Err(payload) = second.join() {
        std::panic::resume_unwind(payload);
    }
}
