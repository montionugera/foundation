//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::future::Future as _;
use std::pin::pin;
use std::ptr::{self, NonNull};
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::time::Span;

use crate::connection::Manager;
use crate::ffi::{self, Status};

#[cfg(test)]
mod tests;

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
    answer: Box<Answer>,
    /// Drops after `raw` is deleted, as it holds the loop of the client.
    connections: Manager,
}

/// The answer of the read in flight.
#[derive(Default)]
struct Answer {
    /// The request of the read in flight. The callback drops the answer of another
    /// request, which is a read whose future dropped.
    request: Cell<Option<u32>>,
    value: Cell<Option<Result<f64, Status>>>,
}

impl Client {
    /// Makes a client on its own manager over `clock`, `net`, and `rng`, connects it to
    /// the server at the endpoint `url` with the security mode `None` and an anonymous
    /// session, and gives it once the session is active. On an error, it closes the
    /// session that it made.
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
        let raw = unsafe { ffi::shim_client_new(connections.events().raw()) };
        let raw = NonNull::new(raw).expect("open62541 refused the client");
        let this = Self {
            raw,
            answer: Box::default(),
            connections,
        };
        // SAFETY: the client lives.
        let timeout_ms = unsafe { ffi::shim_client_timeout(this.raw.as_ptr()) };
        let mut deadline =
            pin!(clock.sleep(Span::from_nanos(i64::from(timeout_ms) * 1_000_000)));
        // SAFETY: the client lives, and copies the URL. The drive reads the status that
        // it gives from the state of the client.
        unsafe { ffi::UA_Client_connectAsync(this.raw.as_ptr(), url.as_ptr()) };
        let connected = this
            .connections
            .drive(|cx| {
                this.connections.events().run();
                let (_, session, status) = this.state();
                if status != Status::GOOD {
                    Poll::Ready(Err(status))
                } else if session == ffi::Session::ACTIVATED {
                    Poll::Ready(Ok(()))
                } else if deadline.as_mut().poll(cx).is_ready() {
                    Poll::Ready(Err(Status::BAD_TIMEOUT))
                } else {
                    Poll::Pending
                }
            })
            .await;
        match connected {
            Ok(()) => Ok(this),
            Err(status) => {
                this.close().await;
                Err(status)
            }
        }
    }

    /// Reads the Value of `node`, a scalar Double.
    ///
    /// # Errors
    ///
    /// The status of open62541 when it refuses the request, the status of the answer
    /// or of its value, or `BadTypeMismatch` when the value is not a scalar Double.
    pub(crate) async fn read(&mut self, node: Node) -> Result<f64, Status> {
        let mut request = 0;
        // SAFETY: the client lives, and `answer` outlives it.
        let status = Status(unsafe {
            ffi::shim_client_read(
                self.raw.as_ptr(),
                node.namespace,
                node.key,
                answer,
                ptr::from_ref::<Answer>(&self.answer).cast_mut().cast(),
                &raw mut request,
            )
        });
        if status != Status::GOOD {
            return Err(status);
        }
        self.answer.request.set(Some(request));
        self.connections
            .drive(|_| {
                self.connections.events().run();
                self.answer.value.take().map_or(Poll::Pending, Poll::Ready)
            })
            .await
    }

    /// Closes the session and the connection of the client, and waits until the
    /// connection is closed. When the server has not answered the request that makes
    /// the session, it waits for that answer or for its request timeout first.
    pub(crate) async fn close(self) {
        // A CloseSession sent before the CreateSession answer has no session token, so
        // the server keeps the session that it makes.
        self.connections
            .drive(|_| {
                self.connections.events().run();
                if self.state().1 == ffi::Session::CREATE_REQUESTED {
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
                if self.state().0 == ffi::Channel::CLOSED {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
    }

    /// Gives the channel state, the session state, and the connect status of the
    /// client.
    fn state(&self) -> (ffi::Channel, ffi::Session, Status) {
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
        (channel, session, Status(status))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the client lives, and nothing else holds it.
        unsafe { ffi::shim_client_free(self.raw.as_ptr()) };
    }
}

/// Puts the answer of `request` in the [`Answer`] at `data` when it is the read in
/// flight.
///
/// # Safety
///
/// `data` points at a live [`Answer`], and `value` points at the answer when `status`
/// is `Good`.
unsafe extern "C" fn answer(
    _: *mut ffi::Client,
    data: *mut c_void,
    request: u32,
    status: u32,
    value: *mut c_void,
) {
    // SAFETY: the client that holds the answer lives.
    let answer = unsafe { &*data.cast_const().cast::<Answer>() };
    if answer.request.get() != Some(request) {
        return;
    }
    let mut double = 0.0;
    let mut status = Status(status);
    if status == Status::GOOD {
        // SAFETY: open62541 gives the answer with `Good`.
        status = Status(unsafe { ffi::shim_value_double(value, &raw mut double) });
    }
    answer.value.set(Some(if status == Status::GOOD {
        Ok(double)
    } else {
        Err(status)
    }));
}
