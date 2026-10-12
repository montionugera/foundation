//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::ptr::{self, NonNull};
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::time::Span;

use crate::connection::{CHUNKS, Manager, SENDS};
use crate::ffi::{self, Status};

#[cfg(test)]
mod tests;

/// The most reads of a client in flight. A read past it waits until one ends.
const READS: u16 = 16;

const _: () = assert!(
    READS as usize * CHUNKS as usize <= SENDS / 2,
    "the other half of the sends of a connection is for the requests of open62541"
);

/// A numeric node of an OPC UA server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) namespace: u16,
    pub(crate) key: u32,
}

/// A client of open62541 with an active session, on its own loop and connection
/// manager, on the thread that made it. Its loop runs only in its calls, so a client
/// idle for longer than the channel lifetime of its server (often 10 minutes) loses
/// its connection, and each later read fails. A drop closes its connection and waits
/// for nothing, but leaves its session on the server until the session times out; a
/// [`Client::close`] closes the session too.
pub(crate) struct Client {
    raw: NonNull<ffi::Client>,
    /// C holds a pointer to it until `raw` is deleted, which calls the callback of each
    /// read in flight.
    reads: Box<Reads>,
    clock: Clock,
    /// The request timeout of the client.
    timeout: Span,
    /// Drops after `raw` is deleted, as it holds the loop of the client.
    connections: Manager,
}

/// The reads of a client in flight, shared with the callback.
#[derive(Default)]
struct Reads {
    /// The request of the last read that open62541 took, until it times out. The
    /// callback drops the answer of another request, and `read` clears the answer of a
    /// last read whose future dropped.
    request: Cell<Option<u32>>,
    value: Cell<Option<Result<f64, Status>>>,
    /// The reads that open62541 took and has not called back, at most [`READS`].
    pending: Cell<u16>,
}

impl Client {
    /// Makes a client on its own manager over `clock`, `net`, and `rng`, connects it to
    /// the server at the endpoint `url` with the security mode `None` and an anonymous
    /// session, and gives it once the session is active. On an error, it closes the
    /// session that it made with [`Client::close`] first, which can take up to two more
    /// request timeouts.
    ///
    /// # Errors
    ///
    /// `BadTcpEndpointUrlInvalid` when `url` holds a NUL, `BadTimeout` when the session
    /// is not active after the request timeout of the client (5 s), else the status of
    /// open62541 when it refuses `url` or the connect fails.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the client.
    pub(crate) async fn connect(
        clock: Clock,
        net: Net,
        rng: &mut Rng,
        url: &str,
    ) -> Result<Self, Status> {
        let url =
            CString::new(url).map_err(|_nul| Status::BAD_TCP_ENDPOINT_URL_INVALID)?;
        let connections = Manager::new(Clock::clone(&clock), net, rng);
        // SAFETY: the loop outlives the client, which `drop` deletes first.
        let raw = unsafe { ffi::shim_client_new(connections.events().raw(), CHUNKS) };
        let raw = NonNull::new(raw).expect("open62541 refused the client");
        // SAFETY: the client lives.
        let timeout_ms = unsafe { ffi::shim_client_timeout(raw.as_ptr()) };
        let timeout = Span::from_nanos(i64::from(timeout_ms) * 1_000_000);
        let this = Self {
            raw,
            reads: Box::default(),
            clock,
            timeout,
            connections,
        };
        let deadline = this.clock.now() + this.timeout;
        // SAFETY: the client lives, and copies the URL. The drive reads the status that
        // it gives from the state of the client.
        unsafe { ffi::UA_Client_connectAsync(this.raw.as_ptr(), url.as_ptr()) };
        let connected = this
            .connections
            .drive_until(Some(deadline), |_| {
                this.connections.events().run();
                let state = this.state();
                if state.status != Status::GOOD {
                    Poll::Ready(Err(state.status))
                } else if state.session == ffi::Session::ACTIVATED {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })
            .await
            .unwrap_or(Err(Status::BAD_TIMEOUT));
        match connected {
            Ok(()) => Ok(this),
            Err(status) => {
                this.close().await;
                Err(status)
            }
        }
    }

    /// Reads the Value of `node`, a scalar Double. A read that times out or whose
    /// future dropped stays in flight until the server answers it or the connection
    /// closes. While [`READS`] are in flight, a read first waits until one ends.
    ///
    /// # Errors
    ///
    /// `BadTooManyOperations` when no read ends in the request timeout of the client
    /// (5 s), as the server holds the requests of [`READS`] reads: the client sends no
    /// more reads until the server answers one. `BadTimeout` when it has no answer
    /// after that timeout, its wait included, the status of open62541 when it refuses
    /// the request, the status of the answer or of its value, or `BadTypeMismatch` when
    /// the value is not a scalar Double.
    pub(crate) async fn read(&mut self, node: Node) -> Result<f64, Status> {
        let deadline = self.clock.now() + self.timeout;
        if self.reads.pending.get() == READS {
            self.connections
                .drive_until(Some(deadline), |_| {
                    self.connections.events().run();
                    if self.reads.pending.get() < READS {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await
                .ok_or(Status::BAD_TOO_MANY_OPERATIONS)?;
        }
        let mut request = 0;
        // SAFETY: the client lives, and `reads` outlives it.
        let status = Status(unsafe {
            ffi::shim_client_read(
                self.raw.as_ptr(),
                node.namespace,
                node.key,
                answer,
                ptr::from_ref::<Reads>(&self.reads).cast_mut().cast(),
                &raw mut request,
            )
        });
        if status != Status::GOOD {
            return Err(status);
        }
        self.reads.pending.set(self.reads.pending.get() + 1);
        self.reads.request.set(Some(request));
        // It can hold the answer of a read whose future dropped after the callback.
        self.reads.value.set(None);
        let answer = self
            .connections
            .drive_until(Some(deadline), |_| {
                self.connections.events().run();
                self.reads.value.take().map_or(Poll::Pending, Poll::Ready)
            })
            .await;
        answer.unwrap_or_else(|| {
            self.reads.request.set(None);
            Err(Status::BAD_TIMEOUT)
        })
    }

    /// Closes the session and the connection of the client, and waits until the
    /// connection is closed. When the server has not answered the request that makes
    /// the session, it waits for that answer or for its request timeout first. A client
    /// that lost its connection cannot close its session, which stays on the server
    /// until it times out.
    pub(crate) async fn close(self) {
        // The server refuses a CloseSession sent before the CreateSession answer, and
        // open62541 then activates the session, which the server keeps.
        self.connections
            .drive(|_| {
                self.connections.events().run();
                if self.state().session == ffi::Session::CREATE_REQUESTED {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
        // SAFETY: the client lives. On a failure to send, it closes the connection.
        unsafe { ffi::UA_Client_disconnectAsync(self.raw.as_ptr()) };
        self.connections
            .drive(|_| {
                self.connections.events().run();
                if self.state().channel == ffi::Channel::CLOSED {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
    }

    /// Gives the state of the client.
    fn state(&self) -> State {
        let (mut channel, mut session, mut status) =
            (ffi::Channel(0), ffi::Session(0), 0);
        // SAFETY: the client lives.
        unsafe {
            ffi::UA_Client_getState(
                self.raw.as_ptr(),
                &raw mut channel,
                &raw mut session,
                &raw mut status,
            );
        }
        State {
            channel,
            session,
            status: Status(status),
        }
    }
}

/// The state of a client.
struct State {
    channel: ffi::Channel,
    session: ffi::Session,
    /// The connect status.
    status: Status,
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the client lives, and nothing else holds it.
        unsafe { ffi::shim_client_free(self.raw.as_ptr()) };
    }
}

/// Ends one read of the [`Reads`] at `data`, and puts the answer of `request` there
/// when it is the read that waits.
///
/// # Safety
///
/// `data` points at live [`Reads`], and `value` points at the answer when `status`
/// is `Good`.
unsafe extern "C" fn answer(
    _: *mut ffi::Client,
    data: *mut c_void,
    request: u32,
    status: u32,
    value: *mut c_void,
) {
    // SAFETY: the client that holds the reads lives.
    let reads = unsafe { &*data.cast_const().cast::<Reads>() };
    reads.pending.set(reads.pending.get() - 1);
    if reads.request.get() != Some(request) {
        return;
    }
    let mut double = 0.0;
    let mut status = Status(status);
    if status == Status::GOOD {
        // SAFETY: open62541 gives the answer with `Good`.
        status = Status(unsafe { ffi::shim_value_double(value, &raw mut double) });
    }
    reads.value.set(Some(if status == Status::GOOD {
        Ok(double)
    } else {
        Err(status)
    }));
}
