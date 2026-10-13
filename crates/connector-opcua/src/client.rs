//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::mem;
use std::ptr::{self, NonNull};
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::time::Span;

use crate::connection::{CHUNKS, Manager, SENDS};
use crate::ffi::{self, Status};
use crate::node::Node;

#[cfg(test)]
mod tests;

/// The most reads of a client in flight. A read past it waits until one ends.
const READS: u16 = 16;

const _: () = assert!(
    READS as usize * CHUNKS as usize <= SENDS / 2,
    "the other half of the sends of a connection is for the requests of open62541"
);

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
    /// The values of the last read, which [`Client::read`] gives. It swaps them with
    /// [`Reads::values`], so neither grows past the most nodes of a read.
    values: Vec<Result<f64, Status>>,
    clock: Clock,
    /// The request timeout of the client.
    timeout: Span,
    /// Drops after `raw` is deleted, as it holds the loop of the client.
    connections: Manager,
}

/// The reads of a client in flight, shared with the callback.
#[derive(Default)]
struct Reads {
    /// The request of the last read that open62541 took. The callback drops the answer
    /// of another request, and `read` clears the late answer of a last read that timed
    /// out or whose future dropped.
    request: Cell<Option<u32>>,
    /// The status of the answer of the last read: `Good` when [`Reads::values`]
    /// holds its values.
    answer: Cell<Option<Status>>,
    values: Cell<Vec<Result<f64, Status>>>,
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
            values: Vec::new(),
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

    /// Reads the Value of each node of `nodes` in one request, and gives the value of
    /// each node in order: its scalar Double, the bad status of its value, or
    /// `BadTypeMismatch` for another value. A read that times out or whose future
    /// dropped stays in flight until the server answers it or the connection closes.
    /// While [`READS`] are in flight, a read first waits until one ends.
    ///
    /// # Errors
    ///
    /// `BadTooManyOperations` when no read ends in the request timeout of the client
    /// (5 s), as the server holds the requests of [`READS`] reads: the client sends no
    /// more reads until the server answers one. `BadTimeout` when it has no answer
    /// after that timeout, its wait included, the status of open62541 when it refuses
    /// the request, the status of the answer, or `BadUnexpectedError` when the answer
    /// has a value for more or fewer nodes than `nodes`.
    pub(crate) async fn read(
        &mut self,
        nodes: &Nodes,
    ) -> Result<&[Result<f64, Status>], Status> {
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
        // SAFETY: the client and `nodes` live, and `reads` outlives the client.
        let status = Status(unsafe {
            ffi::shim_client_read(
                self.raw.as_ptr(),
                nodes.raw.as_ptr(),
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
        // It can hold the late answer of the last read.
        self.reads.answer.set(None);
        let status = self
            .connections
            .drive_until(Some(deadline), |_| {
                self.connections.events().run();
                self.reads.answer.take().map_or(Poll::Pending, Poll::Ready)
            })
            .await
            .unwrap_or(Status::BAD_TIMEOUT);
        if status != Status::GOOD {
            return Err(status);
        }
        let values = self.reads.values.take();
        self.reads
            .values
            .set(mem::replace(&mut self.values, values));
        if self.values.len() != nodes.count {
            return Err(Status::BAD_UNEXPECTED_ERROR);
        }
        Ok(&self.values)
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

/// The Read request of a list of nodes, built once.
pub(crate) struct Nodes {
    raw: NonNull<ffi::ReadRequest>,
    count: usize,
}

impl Nodes {
    /// Builds one Read request of the Value of each node of `nodes`, in order.
    ///
    /// # Panics
    ///
    /// If open62541 cannot allocate the request.
    pub(crate) fn new(nodes: &[Node]) -> Self {
        let raw: Vec<ffi::Node<'_>> = nodes.iter().map(Into::into).collect();
        // SAFETY: `raw` holds `nodes.len()` nodes, and C copies each name.
        let request = unsafe { ffi::shim_read_new(raw.as_ptr(), raw.len()) };
        Self {
            raw: NonNull::new(request).expect("open62541 allocates the request"),
            count: nodes.len(),
        }
    }

    /// Gives the request, which lives while `self` does.
    pub(crate) fn raw(&self) -> *mut ffi::ReadRequest {
        self.raw.as_ptr()
    }
}

impl Drop for Nodes {
    fn drop(&mut self) {
        // SAFETY: the request lives, and nothing else holds it.
        unsafe { ffi::shim_read_free(self.raw.as_ptr()) };
    }
}

/// Ends one read of the [`Reads`] at `data`, and puts the answer of `request` there
/// when it is the read that waits.
///
/// # Safety
///
/// `data` points at live [`Reads`], and `response` at the answer.
unsafe extern "C" fn answer(
    _: *mut ffi::Client,
    data: *mut c_void,
    request: u32,
    response: *mut ffi::ReadResponse,
) {
    // SAFETY: the client that holds the reads lives.
    let reads = unsafe { &*data.cast_const().cast::<Reads>() };
    reads.pending.set(reads.pending.get() - 1);
    if reads.request.get() != Some(request) {
        return;
    }
    // SAFETY: open62541 gives a live response.
    let status = Status(unsafe { ffi::shim_response_result(response.cast()) });
    if status == Status::GOOD {
        let mut values = reads.values.take();
        values.clear();
        // SAFETY: open62541 gives a live response.
        let count = unsafe { ffi::shim_read_count(response) };
        values.extend((0..count).map(|index| {
            let mut double = 0.0;
            // SAFETY: the response lives and has more results than `index`.
            let value = unsafe { ffi::shim_read_result(response, index) };
            // SAFETY: the value lives with the response.
            let status =
                Status(unsafe { ffi::shim_value_double(value, &raw mut double) });
            if status == Status::GOOD {
                Ok(double)
            } else {
                Err(status)
            }
        }));
        reads.values.set(values);
    }
    reads.answer.set(Some(status));
}
