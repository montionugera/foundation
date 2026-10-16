//! A UDP socket: a Tokio socket, with the calls of `noq-udp`.

use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::thread::{self, ThreadId};

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

/// The OS code of a socket option that the OS does not have.
#[cfg(target_os = "linux")]
const NOPROTOOPT: i32 = 92;
#[cfg(target_vendor = "apple")]
const NOPROTOOPT: i32 = 42;

/// The OS code of an invalid argument.
const INVAL: i32 = 22;

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
compile_error!("the sdk runs only on Linux and macOS until #2923 adds Windows");

/// A bound UDP socket.
pub(super) struct Udp(Arc<Bound>);

/// What each half of one socket reads.
struct Bound {
    socket: UdpSocket,
    state: UdpSocketState,
    local: SocketAddr,
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
    });
    let receiver = Receiver {
        bound: Arc::clone(&bound),
        thread: None,
    };
    Ok((Udp(bound), receiver))
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
            writable: None,
            thread: None,
        })
    }
}

/// A wait until the socket is writable.
type Writable = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

/// The driver of one `Sender` clone.
struct Sender {
    bound: Arc<Bound>,
    /// The wait of this clone after the send buffer was full. Each clone waits on its
    /// own: Tokio wakes only the last waker of `poll_send_ready`.
    writable: Option<Writable>,
    /// The thread of the first poll.
    thread: Option<ThreadId>,
}

impl sender::Driver for Sender {
    fn poll_send(
        &mut self,
        cx: &mut Context<'_>,
        transmit: &Transmit<'_>,
    ) -> Poll<Result<(), Error>> {
        on_thread("UDP sender", &mut self.thread);
        let bound = &*self.bound;
        let remote = transmit.destination;
        let (destination, src_ip) = route(bound.local, transmit)?;
        let datagram = noq_udp::Transmit {
            destination,
            ecn: transmit.ecn.map(to_codepoint),
            contents: transmit.contents,
            segment_size: None,
            src_ip,
        };
        loop {
            if let Some(writable) = &mut self.writable {
                ready!(writable.as_mut().poll(cx)).map_err(|e| from_io(&e))?;
                self.writable = None;
            }
            let outcome = bound.socket.try_io(Interest::WRITABLE, || {
                bound.state.try_send((&bound.socket).into(), &datagram)
            });
            if let Poll::Ready(sent) = sent(outcome, remote) {
                return Poll::Ready(sent);
            }
            let owned = Arc::clone(&self.bound);
            self.writable =
                Some(Box::pin(async move { owned.socket.writable().await }));
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
///
/// # Errors
///
/// [`Error::Unreachable`] when the socket is bound to an address of the other family,
/// and the error of `EINVAL` for a source that the socket cannot send from.
fn route(
    local: SocketAddr,
    transmit: &Transmit<'_>,
) -> Result<(SocketAddr, Option<IpAddr>), Error> {
    let remote = transmit.destination;
    let destination = if local.is_ipv6() {
        canonical(remote)
    } else {
        remote
    };
    let any = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    if local.ip() != any && local.is_ipv4() != destination.is_ipv4() {
        return Err(Error::Unreachable { remote });
    }
    if transmit.source.is_some_and(|source| {
        let canonical = source.to_canonical();
        canonical.is_unspecified()
            || local.is_ipv4() && source.is_ipv6()
            || destination.is_ipv4() && canonical.is_ipv6()
    }) {
        return Err(Error::Io { code: INVAL });
    }
    if local.is_ipv4() {
        return Ok((remote, transmit.source));
    }
    let mapped = |ip: IpAddr| match ip {
        IpAddr::V4(v4) => IpAddr::V6(v4.to_ipv6_mapped()),
        IpAddr::V6(_) => ip,
    };
    let destination = match destination {
        SocketAddr::V4(v4) => SocketAddr::new(mapped(IpAddr::V4(*v4.ip())), v4.port()),
        SocketAddr::V6(_) => destination,
    };
    Ok((destination, transmit.source.map(mapped)))
}

/// The driver of the `Receiver`.
pub(super) struct Receiver {
    bound: Arc<Bound>,
    /// The thread of the first poll.
    thread: Option<ThreadId>,
}

impl receiver::Driver for Receiver {
    fn poll_recv(
        &mut self,
        cx: &mut Context<'_>,
        buffers: &mut [IoSliceMut<'_>],
        meta: &mut [Meta],
    ) -> Poll<Result<usize, Error>> {
        on_thread("UDP receiver", &mut self.thread);
        let bound = &*self.bound;
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

/// Records the thread of the first poll of a `kind` in `thread`.
///
/// # Panics
///
/// On a thread other than that of the first poll, as the contract of `env::net::udp`
/// asks.
fn on_thread(kind: &str, thread: &mut Option<ThreadId>) {
    let first = *thread.get_or_insert_with(|| thread::current().id());
    assert_eq!(
        first,
        thread::current().id(),
        "a {kind} polls only on the thread of its first poll"
    );
}

/// `address`, with an IPv4 address mapped in IPv6 given as IPv4. An IPv6 address keeps
/// its scope and flow label, so a reply matches its dial.
fn canonical(address: SocketAddr) -> SocketAddr {
    match address {
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => SocketAddr::new(v4.into(), v6.port()),
            None => address,
        },
        SocketAddr::V4(_) => address,
    }
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
