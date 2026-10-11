//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::ffi::{CString, c_void};
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;

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
/// manager, on the thread that made it. A drop closes its connection and waits for
/// nothing.
pub(crate) struct Client {
    raw: NonNull<ffi::Client>,
    /// Drops after `raw` is deleted, as it holds the loop of the client.
    connections: Manager,
}

/// The answer of a read. C holds one count from a read until its callback.
type Slot = Cell<Option<Result<f64, Status>>>;

impl Client {
    /// Makes a client on its own manager over `clock`, `net`, and `rng`, connects it to
    /// the server at the endpoint `url` with the security mode `None` and an anonymous
    /// session, and gives it once the session is active.
    ///
    /// # Errors
    ///
    /// `BadTcpEndpointUrlInvalid` when `url` holds a NUL, else the status of open62541
    /// when it refuses `url` or the connect fails.
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
        let connections = Manager::new(clock, net, rng);
        // SAFETY: the loop outlives the client, which `drop` deletes first.
        let raw = unsafe { ffi::shim_client_new(connections.events().raw()) };
        let raw = NonNull::new(raw).expect("open62541 refused the client");
        let this = Self { raw, connections };
        // SAFETY: the client lives, and copies the URL.
        let status = Status(unsafe {
            ffi::UA_Client_connectAsync(this.raw.as_ptr(), url.as_ptr())
        });
        if status != Status::GOOD {
            return Err(status);
        }
        this.connections
            .drive(|_| {
                this.connections.events().run();
                let (session, status) = this.state();
                if status != Status::GOOD {
                    Poll::Ready(Err(status))
                } else if session == ffi::Session::ACTIVATED {
                    Poll::Ready(Ok(()))
                } else {
                    Poll::Pending
                }
            })
            .await?;
        Ok(this)
    }

    /// Reads the Value of `node`, a scalar Double.
    ///
    /// # Errors
    ///
    /// The status of open62541 when it refuses the request, the status of the answer
    /// or of its value, or `BadTypeMismatch` when the value is not a scalar Double.
    pub(crate) async fn read(&mut self, node: Node) -> Result<f64, Status> {
        let slot = Rc::new(Slot::new(None));
        let data = Rc::into_raw(Rc::clone(&slot));
        // SAFETY: the client lives. `answer` takes the count at `data`.
        let status = Status(unsafe {
            ffi::shim_client_read(
                self.raw.as_ptr(),
                node.namespace,
                node.key,
                answer,
                data.cast_mut().cast(),
            )
        });
        if status != Status::GOOD {
            // SAFETY: open62541 refused the request, so it never calls `answer`.
            drop(unsafe { Rc::from_raw(data) });
            return Err(status);
        }
        self.connections
            .drive(|_| {
                self.connections.events().run();
                slot.take().map_or(Poll::Pending, Poll::Ready)
            })
            .await
    }

    /// Gives the session state and the connect status of the client.
    fn state(&self) -> (ffi::Session, Status) {
        let (mut session, mut status) = (ffi::Session(0), 0);
        // SAFETY: the client lives, and the call writes each output that is not null.
        unsafe {
            ffi::UA_Client_getState(
                self.raw.as_ptr(),
                ptr::null_mut(),
                &raw mut session,
                &raw mut status,
            );
        }
        (session, Status(status))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: the client lives, and nothing else holds it.
        unsafe { ffi::shim_client_free(self.raw.as_ptr()) };
    }
}

/// Puts the answer of a read in its slot.
///
/// # Safety
///
/// `data` holds the count of a slot that `read` gave, and `value` points at the answer
/// when `status` is `Good`. open62541 calls it once for each request.
unsafe extern "C" fn answer(
    _: *mut ffi::Client,
    data: *mut c_void,
    _: u32,
    status: u32,
    value: *mut c_void,
) {
    // SAFETY: `read` gave this call one count of the slot.
    let slot = unsafe { Rc::from_raw(data.cast_const().cast::<Slot>()) };
    let mut double = 0.0;
    let mut status = Status(status);
    if status == Status::GOOD {
        // SAFETY: open62541 gives the answer with `Good`.
        status = Status(unsafe { ffi::shim_value_double(value, &raw mut double) });
    }
    slot.set(Some(if status == Status::GOOD {
        Ok(double)
    } else {
        Err(status)
    }));
}
