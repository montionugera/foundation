//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::ffi::{CStr, c_void};
use std::mem;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::sample::Value;
use types::time::Span;

use crate::alloc;
use crate::connection::{CHUNKS, Manager, SENDS};
use crate::event::Time;
use crate::ffi::{self, Status};
use crate::node::Node;

#[cfg(test)]
mod tests;

/// The most reads of a client in flight. A read past it waits until one ends.
const READS: u16 = 16;

/// The bytes of the headers of a chunk with the security mode None: the message header
/// (8), the channel (4), the token (4), and the sequence header (8).
const HEADER: usize = 24;

/// The most bytes that open62541 leaves empty at the end of a chunk of a Read but the
/// last. It moves a value of a fixed size whole to the next chunk and splits a string,
/// so this is 1 less than the largest value of a fixed size in a Read today, a Double
/// or a 64-bit integer (8 bytes). A GUID key (19 bytes) would need 18.
const SPLIT: usize = 7;

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
    /// [`Reads::values`]. An answer with more values than nodes fails before it is
    /// copied, so neither grows past the most nodes of a read.
    values: Vec<Result<Value, Status>>,
    clock: Clock,
    /// The request timeout of the client.
    timeout: Span,
    /// The most bytes of the body of a message that the client sends.
    sends: usize,
    /// The most bytes of the body of a message that the client takes.
    takes: usize,
    /// The encoded bytes of the session token.
    token: usize,
    /// Tells the [`Nodes`] that this client checked from those of another.
    key: Rc<()>,
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
    /// The count of nodes of the last read.
    count: Cell<usize>,
    /// The status of the answer of the last read: `Good` when [`Reads::values`]
    /// holds its values.
    answer: Cell<Option<Status>>,
    values: Cell<Vec<Result<Value, Status>>>,
    /// The reads that open62541 took and has not called back, at most [`READS`].
    pending: Cell<u16>,
}

impl Client {
    /// Makes a client on its own manager over `time`, `net`, and `rng`, connects it to
    /// the server at the endpoint `url` with the security mode `None` and an anonymous
    /// session, and gives it once the session is active. On an error, it closes the
    /// session that it made with [`Client::close`] first, which can take up to two
    /// more request timeouts.
    ///
    /// # Errors
    ///
    /// `BadTimeout` when the session is not active after the request timeout of the
    /// client (5 s), `BadConnectionClosed` when the server closes or resets an open
    /// stream before the session is active, with no new dial after that close, else the
    /// status of open62541 when it refuses `url` or the connect fails.
    ///
    /// # Aborts
    ///
    /// When open62541 cannot allocate the loop, the connection manager, or the
    /// client, as [`alloc::exhausted`] says.
    pub(crate) async fn connect(
        time: Time,
        net: Net,
        rng: &mut Rng,
        url: &CStr,
    ) -> Result<Self, Status> {
        let clock = Clock::clone(&time.clock);
        let connections = Manager::new(time, net, rng);
        // SAFETY: the loop outlives the client, which `drop` deletes first.
        let raw = unsafe { ffi::shim_client_new(connections.events().raw(), CHUNKS) };
        let Some(raw) = NonNull::new(raw) else {
            alloc::exhausted()
        };
        // SAFETY: `raw` is a new client of the loop of `connections`.
        unsafe { Self::start(clock, connections, raw, url) }.await
    }

    /// Connects `raw` as [`Client::connect`] does.
    ///
    /// # Safety
    ///
    /// `raw` comes from `shim_client_new` on the loop of `connections`, has not
    /// connected, and nothing else owns it: the client frees it.
    async unsafe fn start(
        clock: Clock,
        connections: Manager,
        raw: NonNull<ffi::Client>,
        url: &CStr,
    ) -> Result<Self, Status> {
        // SAFETY: the client lives.
        let timeout_ms = unsafe { ffi::shim_client_timeout(raw.as_ptr()) };
        let timeout = Span::from_nanos(i64::from(timeout_ms) * 1_000_000);
        let mut this = Self {
            raw,
            reads: Box::default(),
            values: Vec::new(),
            clock,
            timeout,
            sends: 0,
            takes: 0,
            token: 0,
            key: Rc::default(),
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
            Ok(()) => {
                let mut limits = [0; 5];
                // SAFETY: the client lives.
                let status = Status(unsafe {
                    ffi::shim_client_limits(this.raw.as_ptr(), &raw mut limits)
                });
                assert_eq!(status, Status::GOOD, "open62541 gives the limits");
                let [send, receive, message, chunks, token] = limits;
                (this.sends, this.takes) =
                    (most(send, message, chunks), most(receive, message, chunks));
                this.token = token;
                Ok(this)
            }
            Err(status) => {
                this.close().await;
                Err(status)
            }
        }
    }

    /// Builds one Read request of the Value of each node of `nodes`, in order, for
    /// this client alone.
    ///
    /// # Errors
    ///
    /// [`Large`] when the request, or a good answer with each field of a Double, the
    /// largest scalar, for each node, needs more bytes than one message of the
    /// session: its chunk size times the smaller of [`CHUNKS`] and the
    /// `MaxChunkCount` of the server, less 7 bytes at the end of each chunk but the
    /// last, or the `MaxMessageSize` of the server when that is smaller. The request
    /// comes first.
    ///
    /// # Aborts
    ///
    /// If open62541 cannot allocate the request, as [`alloc::exhausted`] says.
    pub(crate) fn nodes(&self, nodes: &[Node]) -> Result<Nodes, Large> {
        let request = Nodes::new(nodes, Rc::clone(&self.key));
        // SAFETY: the request lives.
        let ask = unsafe { ffi::shim_read_ask(request.raw(), self.token) };
        // SAFETY: it reads nothing.
        let answer = unsafe { ffi::shim_read_answer(nodes.len()) };
        for (bytes, most) in [(ask, self.sends), (answer, self.takes)] {
            if bytes > most {
                return Err(Large { bytes, most });
            }
        }
        Ok(request)
    }

    /// Reads the Value of each node of `nodes` in one request, and gives the value of
    /// each node in order: its scalar, the status of its value when that is not Good
    /// (a Good with a subcode is Good), or `BadTypeMismatch` for an array or a type
    /// that is not one of [`Value`]. A read of no nodes gives no values and sends
    /// nothing. A read that times out or whose future dropped stays in flight
    /// until the server answers it or the connection closes. While [`READS`] are in
    /// flight, a read first waits until one ends.
    ///
    /// # Errors
    ///
    /// `BadTooManyOperations` when no read ends in the request timeout of the client
    /// (5 s), as the server holds the requests of [`READS`] reads: the client sends no
    /// more reads until the server answers one. `BadTimeout` when it has no answer
    /// after that timeout, its wait included, the status of open62541 when it refuses
    /// the request, the status of the answer, or `BadUnexpectedError` when the answer
    /// has a value for more or fewer nodes than `nodes`.
    ///
    /// # Panics
    ///
    /// If another client built `nodes`.
    pub(crate) async fn read(
        &mut self,
        nodes: &Nodes,
    ) -> Result<&[Result<Value, Status>], Status> {
        assert!(
            Rc::ptr_eq(&nodes.client, &self.key),
            "invariant: a client reads only the nodes that it checked"
        );
        if nodes.count == 0 {
            return Ok(&[]);
        }
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
        self.reads.count.set(nodes.count);
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

/// A Read request past one message of a session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Large {
    /// The bytes of the body of its message.
    pub(crate) bytes: usize,
    /// The most bytes of the body of one message.
    pub(crate) most: usize,
}

/// Gives the most bytes of the body of one message of a Read in chunks of `buffer`
/// bytes, with the `MaxMessageSize` `message` and the `MaxChunkCount` `chunks` of the
/// server, each 0 for none.
fn most(buffer: usize, message: usize, chunks: usize) -> usize {
    let local = usize::try_from(CHUNKS).expect("invariant: a usize holds CHUNKS");
    let chunks = if chunks == 0 {
        local
    } else {
        chunks.min(local)
    };
    let most = chunks * (buffer - HEADER) - (chunks - 1) * SPLIT;
    if message == 0 {
        most
    } else {
        most.min(message)
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
    /// The [`Client::key`] of the client that checked it.
    client: Rc<()>,
}

impl Nodes {
    /// As [`Nodes::new`], for a bench: [`Client::read`] panics on it.
    #[cfg(feature = "sim")]
    pub(crate) fn unchecked(nodes: &[Node]) -> Self {
        Self::new(nodes, Rc::default())
    }

    /// Builds one Read request of the Value of each node of `nodes`, in order, for the
    /// client of `client`, with no check of its size: [`Client::nodes`] checks it.
    ///
    /// # Aborts
    ///
    /// If open62541 cannot allocate the request, as [`alloc::exhausted`] says.
    fn new(nodes: &[Node], client: Rc<()>) -> Self {
        let raw: Vec<ffi::Node<'_>> = nodes.iter().map(Into::into).collect();
        // SAFETY: `raw` holds `nodes.len()` nodes, and C copies each name.
        let request = unsafe { ffi::shim_read_new(raw.as_ptr(), raw.len()) };
        let Some(raw) = NonNull::new(request) else {
            alloc::exhausted()
        };
        Self {
            raw,
            count: nodes.len(),
            client,
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
    let mut status = Status(unsafe { ffi::shim_response_result(response.cast()) });
    // SAFETY: open62541 gives a live response.
    let count = unsafe { ffi::shim_read_count(response) };
    if status == Status::GOOD && count != reads.count.get() {
        status = Status::BAD_UNEXPECTED_ERROR;
    }
    if status == Status::GOOD {
        let mut values = reads.values.take();
        values.clear();
        values.extend((0..count).map(|index| {
            // SAFETY: the response lives and has more results than `index`.
            let value = unsafe { ffi::shim_read_result(response, index) };
            // SAFETY: the value lives with the response.
            unsafe { crate::value::of(value) }
        }));
        reads.values.set(values);
    }
    reads.answer.set(Some(status));
}

/// Whether open62541 reads `url` as an endpoint URL with a host.
pub(crate) fn endpoint(url: &str) -> bool {
    let url = ffi::Bytes {
        length: url.len(),
        data: url.as_ptr().cast_mut(),
    };
    let empty = || ffi::Bytes {
        length: 0,
        data: ptr::null_mut(),
    };
    let (mut host, mut path, mut port) = (empty(), empty(), 0);
    // SAFETY: `url` points at `length` bytes that live through the call, which only
    // reads them. It sets `host` and `path` to point into them, and they are not
    // freed.
    let status = unsafe {
        ffi::UA_parseEndpointUrl(
            &raw const url,
            &raw mut host,
            &raw mut port,
            &raw mut path,
        )
    };
    Status(status) == Status::GOOD && host.length > 0
}
