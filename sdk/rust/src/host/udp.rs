//! A UDP socket: a Tokio socket, with the calls of `noq-udp`.

use std::io::{self, IoSliceMut};
use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, ready};

use env::net::udp::{self, Meta, Transmit, receiver, sender};
use env::net::{Ecn, Error};
use noq_udp::{EcnCodepoint, RecvMeta, UdpSocketState};
use socket2::{Domain, Protocol, Type};
use tokio::io::Interest;
use tokio::net::UdpSocket;

/// The OS code of a datagram over the path MTU.
#[cfg(target_os = "linux")]
const MSGSIZE: i32 = 90;
#[cfg(target_vendor = "apple")]
const MSGSIZE: i32 = 40;
#[cfg(windows)]
const MSGSIZE: i32 = 10_040;

/// The OS code of a socket option that the OS does not have.
#[cfg(target_os = "linux")]
const NOPROTOOPT: i32 = 92;
#[cfg(target_vendor = "apple")]
const NOPROTOOPT: i32 = 42;
#[cfg(windows)]
const NOPROTOOPT: i32 = 10_042;

/// A bound UDP socket.
pub(super) struct Udp(Arc<Bound>);

/// What each half of one socket reads.
struct Bound {
    socket: UdpSocket,
    state: UdpSocketState,
    local: SocketAddr,
    /// Set while a sender lives that polled. Tokio wakes only the last waker that
    /// waits for writable, so a second sender could wait forever.
    claimed: AtomicBool,
}

/// Binds a socket to `config.local`, with don't-fragment set, and gives it with the
/// driver of its one receiver. Call it inside the client's runtime.
pub(super) fn bind(config: &udp::Config) -> Result<(Udp, Receiver), Error> {
    let local = config.local;
    let socket = socket2::Socket::new(
        Domain::for_address(local),
        Type::DGRAM,
        Some(Protocol::UDP),
    )
    .map_err(|e| from_io(&e))?;
    socket
        .set_send_buffer_size(config.send_buffer_bytes)
        .map_err(|e| from_io(&e))?;
    socket
        .set_recv_buffer_size(config.recv_buffer_bytes)
        .map_err(|e| from_io(&e))?;
    if local.is_ipv6() {
        socket.set_only_v6(false).map_err(|e| from_io(&e))?;
    }
    socket.set_nonblocking(true).map_err(|e| from_io(&e))?;
    socket.bind(&local.into()).map_err(|e| match e.kind() {
        io::ErrorKind::AddrInUse => Error::AddressInUse { local },
        _ => from_io(&e),
    })?;
    let socket = UdpSocket::from_std(socket.into()).map_err(|e| from_io(&e))?;
    let state = UdpSocketState::new((&socket).into()).map_err(|e| from_io(&e))?;
    if state.may_fragment() {
        return Err(Error::Io { code: NOPROTOOPT });
    }
    let local = socket.local_addr().map_err(|e| from_io(&e))?;
    let bound = Arc::new(Bound {
        socket,
        state,
        local,
        claimed: AtomicBool::new(false),
    });
    Ok((Udp(Arc::clone(&bound)), Receiver(bound)))
}

impl udp::Driver for Udp {
    fn local(&self) -> SocketAddr {
        self.0.local
    }

    fn send_batch_max(&self) -> NonZeroUsize {
        NonZeroUsize::MIN
    }

    fn recv_batch_max(&self) -> NonZeroUsize {
        self.0.state.gro_segments()
    }

    fn sender(&self) -> Box<dyn sender::Driver> {
        Box::new(Sender {
            bound: Arc::clone(&self.0),
            claimed: false,
        })
    }
}

/// The driver of one `Sender` clone.
struct Sender {
    bound: Arc<Bound>,
    /// Whether this clone holds the claim of the socket.
    claimed: bool,
}

impl sender::Driver for Sender {
    fn poll_send(
        &mut self,
        cx: &mut Context<'_>,
        transmit: &Transmit<'_>,
    ) -> Poll<Result<(), Error>> {
        let bound = &*self.bound;
        if !self.claimed {
            let held = bound.claimed.swap(true, Ordering::Relaxed);
            assert!(!held, "the sdk sends from one sender");
            self.claimed = true;
        }
        let remote = transmit.destination;
        let (destination, src_ip) = route(bound.local, transmit);
        let datagram = noq_udp::Transmit {
            destination,
            ecn: transmit.ecn.map(to_codepoint),
            contents: transmit.contents,
            segment_size: None,
            src_ip,
        };
        loop {
            ready!(bound.socket.poll_send_ready(cx)).map_err(|e| from_io(&e))?;
            let outcome = bound.socket.try_io(Interest::WRITABLE, || {
                bound.state.try_send((&bound.socket).into(), &datagram)
            });
            if let Poll::Ready(sent) = sent(outcome, remote) {
                return Poll::Ready(sent);
            }
        }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        if self.claimed {
            self.bound.claimed.store(false, Ordering::Relaxed);
        }
    }
}

/// The outcome of a send to `remote`: `Pending` when the OS send buffer is full. A
/// datagram over the path MTU is lost, not an error.
fn sent(outcome: io::Result<()>, remote: SocketAddr) -> Poll<Result<(), Error>> {
    let Err(e) = outcome else {
        return Poll::Ready(Ok(()));
    };
    Poll::Ready(match e.kind() {
        io::ErrorKind::WouldBlock => return Poll::Pending,
        _ if e.raw_os_error() == Some(MSGSIZE) => Ok(()),
        io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => {
            Err(Error::Unreachable { remote })
        }
        _ => Err(from_io(&e)),
    })
}

/// The destination and source of `transmit` as the socket bound to `local` takes
/// them: an IPv4 address mapped on an IPv6 socket.
fn route(local: SocketAddr, transmit: &Transmit<'_>) -> (SocketAddr, Option<IpAddr>) {
    let mapped = |ip: IpAddr| match ip {
        IpAddr::V4(v4) if local.is_ipv6() => IpAddr::V6(v4.to_ipv6_mapped()),
        _ => ip,
    };
    let destination = transmit.destination;
    (
        SocketAddr::new(mapped(destination.ip()), destination.port()),
        transmit.source.map(mapped),
    )
}

/// The driver of the `Receiver`.
pub(super) struct Receiver(Arc<Bound>);

impl receiver::Driver for Receiver {
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        buffers: &mut [IoSliceMut<'_>],
        meta: &mut [Meta],
    ) -> Poll<Result<usize, Error>> {
        let bound = &*self.0;
        loop {
            ready!(bound.socket.poll_recv_ready(cx)).map_err(|e| from_io(&e))?;
            let outcome = bound
                .socket
                .try_io(Interest::READABLE, || bound.receive(buffers, meta));
            if let Poll::Ready(received) = received(outcome) {
                return Poll::Ready(received);
            }
        }
    }
}

/// The outcome of a receive: `Pending` when the socket holds no datagram.
fn received(outcome: io::Result<usize>) -> Poll<Result<usize, Error>> {
    match outcome {
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Poll::Pending,
        outcome => Poll::Ready(outcome.map_err(|e| from_io(&e))),
    }
}

impl Bound {
    /// Receives into `buffers` with one call.
    fn receive(
        &self,
        buffers: &mut [IoSliceMut<'_>],
        meta: &mut [Meta],
    ) -> io::Result<usize> {
        let mut received = [RecvMeta::default(); noq_udp::BATCH_SIZE];
        let count = buffers.len().min(received.len());
        let count = self.state.recv(
            (&self.socket).into(),
            &mut buffers[..count],
            &mut received[..count],
        )?;
        for (meta, received) in meta.iter_mut().zip(&received[..count]) {
            *meta = Meta {
                source: canonical(received.addr),
                destination: received.dst_ip.map(|ip| ip.to_canonical()),
                ecn: received.ecn.map(from_codepoint),
                len: received.len,
                stride: received.stride,
            };
        }
        Ok(count)
    }
}

/// `address`, with an IPv4 address mapped in IPv6 given as IPv4.
fn canonical(address: SocketAddr) -> SocketAddr {
    SocketAddr::new(address.ip().to_canonical(), address.port())
}

/// The error of a socket call that failed with `error`.
fn from_io(error: &io::Error) -> Error {
    match error.raw_os_error() {
        Some(code) => Error::Io { code },
        None => panic!("a socket call failed with no OS code: {error}"),
    }
}

fn to_codepoint(ecn: Ecn) -> EcnCodepoint {
    match ecn {
        Ecn::Ect0 => EcnCodepoint::Ect0,
        Ecn::Ect1 => EcnCodepoint::Ect1,
        Ecn::Ce => EcnCodepoint::Ce,
    }
}

fn from_codepoint(ecn: EcnCodepoint) -> Ecn {
    match ecn {
        EcnCodepoint::Ect0 => Ecn::Ect0,
        EcnCodepoint::Ect1 => Ecn::Ect1,
        EcnCodepoint::Ce => Ecn::Ce,
    }
}

#[cfg(test)]
mod tests;
