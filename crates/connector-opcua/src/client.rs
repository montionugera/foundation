//! A client of an OPC UA server, on its own loop and connection manager.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use std::mem;
use std::num::NonZeroUsize;
use std::ptr::{self, NonNull};
use std::rc::Rc;
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;
use types::time::Span;

use crate::alloc;
use crate::connection::{CHUNKS, Manager, SENDS};
use crate::event::Time;
use crate::ffi::{self, Status};
use crate::node::{Key, Node};
use crate::value::Value;

#[cfg(test)]
mod tests;

/// The most requests of a client in flight. A request past it waits until one ends.
const READS: u16 = 16;

/// The bytes of the headers of a chunk with the security mode None: the message header
/// (8), the channel (4), the token (4), and the sequence header (8).
const HEADER: usize = 24;

/// The most bytes that open62541 leaves empty at the end of a chunk of a Read but the
/// last. It moves a value of a fixed size whole to the next chunk and splits a string,
/// so this is 1 less than the largest value of a fixed size in a Read today, a Double
/// or a 64-bit integer (8 bytes). A GUID key (19 bytes) would need 18.
const SPLIT: usize = 7;

/// The value of a slot that no read has written yet. No read gives it: a read ends only
/// when each of its requests has its answer, which writes each of its values.
const UNREAD: Result<Value, Status> = Err(Status::BAD_UNEXPECTED_ERROR);

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
    /// The `MaxNodesPerRead` of the server, the most nodes of one Read request.
    per_read: Option<NonZeroUsize>,
    /// Tells the [`Nodes`] that this client checked from those of another.
    key: Rc<()>,
    /// Drops after `raw` is deleted, as it holds the loop of the client.
    connections: Manager,
}

/// The reads of a client in flight, shared with the callback.
#[derive(Default)]
struct Reads {
    /// The requests of the last read that open62541 took and has not answered. The
    /// callback drops the answer of another request, so a read clears them when it
    /// starts.
    requests: RefCell<Vec<Request>>,
    /// The status of the first failed answer of the last read.
    failed: Cell<Option<Status>>,
    /// The values of the last read, in node order.
    values: Cell<Vec<Result<Value, Status>>>,
    /// The requests that open62541 took and has not called back, of each read, at most
    /// [`READS`].
    pending: Cell<u16>,
}

/// A request of a read in flight.
#[derive(Clone, Copy)]
struct Request {
    /// The number that open62541 gave it.
    key: u32,
    slots: Slots,
}

/// The nodes of one request of a read: the index of the first in the list, and their
/// count.
#[derive(Clone, Copy)]
struct Slots {
    first: usize,
    count: usize,
}

impl Client {
    /// Makes a client on its own manager over `time`, `net`, and `rng`, connects it to
    /// the server at the endpoint `url` with the security mode `None` and an anonymous
    /// session, and gives it once the session is active and it has read the
    /// `MaxNodesPerRead` of the server. On an error, it closes the session that it made
    /// with [`Client::close`] first, which can take up to two more request timeouts.
    ///
    /// # Errors
    ///
    /// `BadTimeout` when the session is not active after the request timeout of the
    /// client (5 s), `BadConnectionClosed` when the server closes or resets an open
    /// stream before the session is active, with no new dial after that close, else the
    /// status of open62541 when it refuses `url` or the connect fails, else the error
    /// of [`Client::read`] when the read of `MaxNodesPerRead` fails.
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
            per_read: None,
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
        let per_read = match connected {
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
                this.per_read().await
            }
            Err(status) => Err(status),
        };
        match per_read {
            Ok(per_read) => {
                this.per_read = per_read;
                Ok(this)
            }
            Err(status) => {
                this.close().await;
                Err(status)
            }
        }
    }

    /// Reads the `MaxNodesPerRead` of the server: a `UInt32` above 0, else none, also
    /// for a status that is not Good, as a server can lack the node.
    async fn per_read(&mut self) -> Result<Option<NonZeroUsize>, Status> {
        let node = Node {
            namespace: 0,
            key: Key::Number(ffi::MAX_NODES_PER_READ),
        };
        let nodes = self
            .nodes(&[node])
            .expect("invariant: a read of one node by number fits one message");
        Ok(match self.read(&nodes).await?[0] {
            Ok(Value::U32(most)) => NonZeroUsize::new(
                usize::try_from(most).expect("invariant: a usize holds a u32"),
            ),
            _ => None,
        })
    }

    /// Builds the Read requests of the Value of each node of `nodes`, for this client
    /// alone. Each request takes the next nodes in order while it has at most the
    /// `MaxNodesPerRead` of the server, and while it and a good answer with each field
    /// of a Double, the largest scalar, for each of its nodes fit one message of the
    /// session: its chunk size times the smaller of [`CHUNKS`] and the
    /// `MaxChunkCount` of the server, less 7 bytes at the end of each chunk but the
    /// last, or the `MaxMessageSize` of the server when that is smaller.
    ///
    /// # Errors
    ///
    /// [`Large`] for the first node whose request alone, or its answer, does not fit
    /// one message. The request comes first.
    ///
    /// # Aborts
    ///
    /// If open62541 cannot allocate a request, as [`alloc::exhausted`] says.
    pub(crate) fn nodes(&self, nodes: &[Node]) -> Result<Nodes, Large> {
        // SAFETY: it reads nothing.
        let empty = unsafe { ffi::shim_read_ask(self.token) };
        // SAFETY: it reads nothing.
        let answer = unsafe { ffi::shim_read_answer() };
        // SAFETY: it reads nothing.
        let result = unsafe { ffi::shim_read_answer_item() };
        let past = |ask: usize, count: usize| {
            let answer = result.saturating_mul(count).saturating_add(answer);
            [(ask, self.sends), (answer, self.takes)]
                .into_iter()
                .find(|&(bytes, most)| bytes > most)
        };
        let zero = Node {
            namespace: 0,
            key: Key::Number(0),
        };
        // SAFETY: `zero` has no name.
        let rest = unsafe { ffi::shim_read_item((&zero).into()) } - id_bytes(&zero);
        let mut counts: Vec<usize> = Vec::new();
        let mut ask = empty;
        for (index, node) in nodes.iter().enumerate() {
            let item = rest + id_bytes(node);
            let count = counts.last().copied();
            let next = count.is_none_or(|count| {
                self.per_read.is_some_and(|most| count == most.get())
                    || past(ask + item, count + 1).is_some()
            });
            if next {
                if let Some((bytes, most)) = past(empty + item, 1) {
                    return Err(Large {
                        node: index,
                        bytes,
                        most,
                    });
                }
                counts.push(0);
                ask = empty;
            }
            ask += item;
            *counts
                .last_mut()
                .expect("invariant: a request takes the node") += 1;
        }
        Ok(Nodes::new(nodes, &counts, Rc::clone(&self.key)))
    }

    /// Reads the Value of each node of `nodes`, and gives the value of each node in
    /// order: its scalar, the status of its value when that is not Good (a Good with a
    /// subcode is Good), or `BadTypeMismatch` for an array or a type that is not one of
    /// [`Value`]. It sends the requests of `nodes` at once, but while [`READS`] are in
    /// flight it sends the next only when one ends. A read of no nodes gives no values
    /// and sends nothing. A request of a read that fails, times out, or whose future
    /// dropped stays in flight until the server answers it or the connection closes.
    ///
    /// # Errors
    ///
    /// `BadTooManyOperations` when no request ends in the request timeout of the client
    /// (5 s) before the first request goes, as the server holds [`READS`] requests:
    /// the client sends no more until the server answers one.
    /// `BadTimeout` when a request has no answer after that timeout, the waits
    /// included. Else the first of: the status of open62541 when it refuses a request,
    /// the status of an answer, or `BadUnexpectedError` when an answer has a value for
    /// more or fewer nodes than its request.
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
        // They can hold the requests and the values of a read that did not end.
        self.reads.requests.borrow_mut().clear();
        self.reads.failed.set(None);
        // Only a growth needs a fill: a good read writes each value.
        let mut values = self.reads.values.take();
        values.resize(nodes.count, UNREAD);
        self.reads.values.set(values);
        // The first requests go before the drive moves the connections on, so a
        // connection that closed since the last read fails them in their answers.
        let mut sent = 0;
        self.send(nodes, &mut sent)?;
        let read = self
            .connections
            .drive_until(Some(deadline), |_| {
                loop {
                    if let Err(status) = self.send(nodes, &mut sent) {
                        return Poll::Ready(Err(status));
                    }
                    self.connections.events().run();
                    if let Some(status) = self.reads.failed.get() {
                        return Poll::Ready(Err(status));
                    }
                    if sent == nodes.parts.len() {
                        return if self.reads.requests.borrow().is_empty() {
                            Poll::Ready(Ok(()))
                        } else {
                            Poll::Pending
                        };
                    }
                    if self.reads.pending.get() == READS {
                        return Poll::Pending;
                    }
                }
            })
            .await;
        match read {
            Some(Ok(())) => {}
            Some(Err(status)) => return Err(status),
            None if sent == 0 => return Err(Status::BAD_TOO_MANY_OPERATIONS),
            None => return Err(Status::BAD_TIMEOUT),
        }
        let values = self.reads.values.take();
        self.reads
            .values
            .set(mem::replace(&mut self.values, values));
        Ok(&self.values)
    }

    /// Sends the requests of `nodes` from the index `sent` while fewer than [`READS`]
    /// are in flight and no answer of this read failed, and moves `sent` past each.
    fn send(&self, nodes: &Nodes, sent: &mut usize) -> Result<(), Status> {
        while let Some(&Part { raw, slots }) = nodes.parts.get(*sent)
            && self.reads.pending.get() < READS
            && self.reads.failed.get().is_none()
        {
            let mut key = 0;
            // SAFETY: the client and `request` live, and `reads` outlives the client.
            let status = Status(unsafe {
                ffi::shim_client_read(
                    self.raw.as_ptr(),
                    raw.as_ptr(),
                    answer,
                    ptr::from_ref::<Reads>(&self.reads).cast_mut().cast(),
                    &raw mut key,
                )
            });
            if status != Status::GOOD {
                return Err(status);
            }
            self.reads.pending.set(self.reads.pending.get() + 1);
            (self.reads.requests.borrow_mut()).push(Request { key, slots });
            *sent += 1;
        }
        Ok(())
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

/// A node whose Read request alone, or its answer, is past one message of a session.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Large {
    /// The index of the node in its list.
    pub(crate) node: usize,
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

/// Gives the bytes of the `NodeId` of `node` in the shortest binary form, the form that
/// open62541 encodes.
fn id_bytes(node: &Node) -> usize {
    match &node.key {
        Key::Number(key) if node.namespace == 0 && *key <= 0xFF => 2,
        Key::Number(key) if node.namespace <= 0xFF && *key <= 0xFFFF => 4,
        Key::Number(_) => 7,
        Key::Name(name) => 7 + name.as_str().len(),
    }
}

/// The Read requests of a list of nodes, built once. It holds at most one request per
/// node: the count of nodes, the `MaxNodesPerRead` of the server, and the message sizes
/// of the session set its length.
pub(crate) struct Nodes {
    /// Each request, in node order.
    parts: Vec<Part>,
    count: usize,
    /// The [`Client::key`] of the client that checked it.
    client: Rc<()>,
}

/// A Read request of a [`Nodes`].
#[derive(Clone, Copy)]
struct Part {
    raw: NonNull<ffi::ReadRequest>,
    slots: Slots,
}

impl Nodes {
    /// As [`Nodes::new`] with one request, for a bench: [`Client::read`] panics on it.
    #[cfg(feature = "sim")]
    pub(crate) fn unchecked(nodes: &[Node]) -> Self {
        Self::new(nodes, &[nodes.len()], Rc::default())
    }

    /// Builds the Read requests of the Value of each node of `nodes`, in order, for the
    /// client of `client`: one for each count of `counts`, which sum to the count of
    /// `nodes`. It checks no size: [`Client::nodes`] does.
    ///
    /// # Aborts
    ///
    /// If open62541 cannot allocate a request, as [`alloc::exhausted`] says.
    fn new(nodes: &[Node], counts: &[usize], client: Rc<()>) -> Self {
        let raw: Vec<ffi::Node<'_>> = nodes.iter().map(Into::into).collect();
        let mut first = 0;
        let parts = counts
            .iter()
            .map(|&count| {
                let part = &raw[first..first + count];
                // SAFETY: `part` holds `count` nodes, and C copies each name.
                let request = unsafe { ffi::shim_read_new(part.as_ptr(), count) };
                let Some(request) = NonNull::new(request) else {
                    alloc::exhausted()
                };
                let slots = Slots { first, count };
                first += count;
                Part {
                    raw: request,
                    slots,
                }
            })
            .collect();
        assert_eq!(first, nodes.len(), "invariant: the counts take each node");
        Self {
            parts,
            count: nodes.len(),
            client,
        }
    }

    /// Gives the count of nodes of each request, in order.
    #[cfg(test)]
    fn counts(&self) -> Vec<usize> {
        self.parts.iter().map(|part| part.slots.count).collect()
    }

    /// Gives the one request of a [`Nodes::unchecked`], which lives while `self` does.
    ///
    /// # Panics
    ///
    /// Unless `self` has exactly one request.
    #[cfg(feature = "sim")]
    pub(crate) fn raw(&self) -> *mut ffi::ReadRequest {
        let [part] = self.parts[..] else {
            panic!(
                "invariant: a bench reads a list of one request, not {}",
                self.parts.len()
            );
        };
        part.raw.as_ptr()
    }
}

impl Drop for Nodes {
    fn drop(&mut self) {
        for part in &self.parts {
            // SAFETY: the request lives, and nothing else holds it.
            unsafe { ffi::shim_read_free(part.raw.as_ptr()) };
        }
    }
}

/// Ends one request of the [`Reads`] at `data`. When it is a request of the last read,
/// puts the values of the answer there, or ends that read with its failed status.
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
    let mut requests = reads.requests.borrow_mut();
    let Some(index) = requests.iter().position(|sent| sent.key == request) else {
        return;
    };
    let Slots { first, count } = requests.swap_remove(index).slots;
    // SAFETY: open62541 gives a live response.
    let mut status = Status(unsafe { ffi::shim_response_result(response.cast()) });
    // SAFETY: open62541 gives a live response.
    if status == Status::GOOD && unsafe { ffi::shim_read_count(response) } != count {
        status = Status::BAD_UNEXPECTED_ERROR;
    }
    if status != Status::GOOD {
        // One run of the events can take more answers of the read after it fails.
        if reads.failed.get().is_none() {
            reads.failed.set(Some(status));
        }
        return;
    }
    let mut values = reads.values.take();
    for (index, value) in values[first..first + count].iter_mut().enumerate() {
        // SAFETY: the response lives and has more results than `index`.
        let result = unsafe { ffi::shim_read_result(response, index) };
        // SAFETY: the value lives with the response.
        *value = unsafe { Value::of(result) };
    }
    reads.values.set(values);
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
