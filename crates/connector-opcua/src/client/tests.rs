use std::cell::Cell;
use std::ffi::CString;
use std::future::poll_fn;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{Context, Poll};

use env::clock::Clock;
use env::net::{Net, Tcp, tcp};
use env::rng::Rng;
use sim::{Sim, node};
use types::time::Span;

use super::{Client, Large, Nodes, Reads, SPLIT, answer, most};
use crate::connection::CHUNKS;
use crate::connection::Manager;
use crate::event;
use crate::ffi::{self, Status};
use crate::node::{Key, Name, Node};
use crate::server::{Limits, Server};
use crate::unwind::caught;

/// The port of the test server.
const PORT: u16 = 4840;

/// The Double variable of the test server.
const VALUE: Node = Node {
    namespace: 1,
    key: Key::Number(1000),
};

/// A `ByteString` whose answer needs 8 chunks, [`CHUNKS`], from [`read_large`].
const FITS: Node = Node {
    namespace: 1,
    key: Key::Number(1002),
};

/// A `ByteString` whose answer needs 9 chunks, one past [`CHUNKS`], from
/// [`read_large`].
const PAST: Node = Node {
    namespace: 1,
    key: Key::Number(1003),
};

/// The size of a chunk, the send buffer size of a channel.
const CHUNK: usize = 1 << 16;

/// The current time of the server, a `DateTime`.
const TIME: Node = Node {
    namespace: 0,
    key: Key::Number(ffi::test::TIME),
};

impl Client {
    /// Reads the Value of `node` alone.
    pub(crate) async fn read_one(&mut self, node: &Node) -> Result<f64, Status> {
        let nodes = self
            .nodes(std::slice::from_ref(node))
            .expect("one node fits");
        self.read(&nodes).await.and_then(|values| values[0])
    }

    /// Builds the Read request of `nodes` for this client with no check of its size.
    fn unchecked(&self, nodes: &[Node]) -> Nodes {
        Nodes::new(nodes, Rc::clone(&self.key))
    }
}

/// The side of a client in a run of [`serve`].
struct Peer<'s> {
    /// The test server.
    server: &'s Server,
    clock: Clock,
    net: Net,
    rng: Rng,
    /// The endpoint of the test server.
    url: String,
    /// The value of [`VALUE`].
    value: f64,
    /// The server does not run while it is set.
    paused: Rc<Cell<bool>>,
}

impl Peer<'_> {
    async fn connect(&mut self, url: &str) -> Result<Client, Status> {
        let url = CString::new(url).expect("the URL holds no NUL");
        let (clock, net) = (Clock::clone(&self.clock), Net::clone(&self.net));
        let wall = event::monotonic(Clock::clone(&clock));
        Client::connect(clock, wall, net, &mut self.rng, &url).await
    }

    async fn connected(&mut self) -> Client {
        let url = self.url.clone();
        self.connect(&url).await.expect("the client connects")
    }

    /// Connects a client whose receive buffer is `receive` bytes.
    async fn receiving(&mut self, receive: u32) -> Client {
        let clock = Clock::clone(&self.clock);
        let wall = event::monotonic(Clock::clone(&clock));
        let connections =
            Manager::new(Clock::clone(&clock), wall, self.net.clone(), &mut self.rng);
        // SAFETY: the loop outlives the client, which the drop of the client deletes
        // first.
        let raw = unsafe { ffi::shim_client_new(connections.events().raw(), CHUNKS) };
        let raw = NonNull::new(raw).expect("open62541 refused the client");
        // SAFETY: the client lives and has not connected.
        unsafe { ffi::test::shim_client_receive(raw.as_ptr(), receive) };
        let url = CString::new(self.url.clone()).expect("the URL holds no NUL");
        // SAFETY: `raw` is a new client of the loop of `connections`.
        let client = unsafe { Client::start(clock, connections, raw, &url) }.await;
        client.expect("the client connects")
    }
}

/// Runs `test` on a node of a run of `seed`, and gives its value and the digest of
/// the run.
fn run<T: Send + 'static>(
    seed: u64,
    test: impl AsyncFnOnce(node::Node) -> T + Send + 'static,
) -> (T, u64) {
    let mut sim = Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(node::Config::default());
    let value = sim
        .run_on(&node, move |node, _| test(node))
        .expect("the run ends");
    (value, sim.digest())
}

/// Starts a test server on `node`, with room for `sessions` sessions and a Double at
/// [`VALUE`] from its random values, runs `body` while the server runs, except while
/// [`Peer::paused`] is set, then closes the server.
async fn serve<T>(
    node: &node::Node,
    sessions: u16,
    body: impl AsyncFnOnce(Peer<'_>) -> T,
) -> T {
    let mut rng = node.entropy().rng();
    let local = SocketAddr::new(node.addresses()[0], PORT);
    let manager = Manager::listening(
        node.clock(),
        event::monotonic(node.clock()),
        node.net(),
        local,
        4,
        &mut rng,
    )
    .expect("the port is free");
    // SAFETY: the close below deletes the server before the manager drops.
    let server = unsafe { Server::start(&manager, c"", sessions) };
    let value = f64::from(u32::try_from(rng.below(1 << 20)).expect("fits"));
    server.add_double(&VALUE, value);
    let peer = Peer {
        server: &server,
        clock: node.clock(),
        net: node.net(),
        rng,
        url: format!("opc.tcp://{local}"),
        value,
        paused: Rc::default(),
    };
    let paused = Rc::clone(&peer.paused);
    let out = {
        let mut server_drive = pin!(manager.drive(|_| {
            manager.events().run();
            Poll::<()>::Pending
        }));
        let mut body = pin!(body(peer));
        poll_fn(|cx| {
            if !paused.get() {
                assert!(server_drive.as_mut().poll(cx).is_pending(), "it never ends");
            }
            body.as_mut().poll(cx)
        })
        .await
    };
    server.close(&manager).await;
    out
}

/// The value of [`VALUE`] and the answer of a read of it, in a run of `seed`.
fn spike(seed: u64) -> ((f64, Result<f64, Status>), u64) {
    run(seed, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            (peer.value, client.read_one(&VALUE).await)
        })
        .await
    })
}

#[test]
fn a_client_reads_a_double_and_a_run_replays() {
    let ((value, read), digest) = spike(7);
    assert_eq!(read, Ok(value));
    assert_eq!(spike(7), ((value, Ok(value)), digest));
    let ((other, read), _) = spike(8);
    assert_eq!(read, Ok(other));
    assert_ne!(other.to_bits(), value.to_bits());
}

#[test]
fn a_read_of_an_unknown_node_fails_with_its_status() {
    let (read, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let unknown = Node {
                key: Key::Number(1001),
                ..VALUE
            };
            client.read_one(&unknown).await
        })
        .await
    });
    assert_eq!(read, Err(Status::BAD_NODE_ID_UNKNOWN));
}

/// The name `node` must reach the server whole: it holds a `;`, an `=`, and a byte
/// past ASCII, and a node whose name is its prefix holds another value. The name in
/// namespace 0 holds no value.
#[test]
fn a_read_of_a_name_node_gives_its_value() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let named = |name: &str| Node {
                namespace: 1,
                key: Key::Name(Name::new(name).expect("a name that is not empty")),
            };
            peer.server.add_double(&named("plant.spike;x=ü"), 2.5);
            peer.server.add_double(&named("plant.spike"), 3.5);
            let mut client = peer.connected().await;
            let reads = (
                client.read_one(&named("plant.spike;x=ü")).await,
                client.read_one(&named("plant.spike")).await,
                client.read_one(&named("plant.dip")).await,
                client
                    .read_one(&Node {
                        namespace: 0,
                        ..named("plant.spike")
                    })
                    .await,
            );
            client.close().await;
            reads
        })
        .await
    });
    let unknown = Err(Status::BAD_NODE_ID_UNKNOWN);
    let expected = (Ok(2.5), Ok(3.5), unknown, unknown);
    assert_eq!(reads, expected);
}

#[test]
fn a_read_of_another_type_fails_with_a_type_mismatch() {
    let (read, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            client.read_one(&TIME).await
        })
        .await
    });
    assert_eq!(read, Err(Status::BAD_TYPE_MISMATCH));
}

#[test]
fn a_read_of_nodes_gives_the_value_of_each_in_order() {
    let ((values, value), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let unknown = Node {
                key: Key::Number(1001),
                ..VALUE
            };
            let nodes = client.nodes(&[TIME, VALUE, unknown, VALUE]).expect("4 fit");
            let values = client.read(&nodes).await.map(<[_]>::to_vec);
            client.close().await;
            (values, peer.value)
        })
        .await
    });
    let expected = [
        Err(Status::BAD_TYPE_MISMATCH),
        Ok(value),
        Err(Status::BAD_NODE_ID_UNKNOWN),
        Ok(value),
    ];
    assert_eq!(values, Ok(expected.to_vec()));
}

/// The late answer of a read of two nodes that timed out, after `dropped` dropped
/// reads. It lands while the next read waits for its own answer, or, at the bound of
/// reads in flight, while that read waits to send.
fn read_after_late_answer(
    dropped: u16,
) -> (Result<Vec<Result<f64, Status>>, Status>, f64) {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.paused.set(true);
            drop_reads(&mut client, dropped).await;
            let late = client.nodes(&[TIME, VALUE]).expect("2 nodes fit");
            assert_eq!(client.read(&late).await, Err(Status::BAD_TIMEOUT));
            peer.paused.set(false);
            let nodes = client.nodes(&[VALUE]).expect("one node fits");
            let values = client.read(&nodes).await.map(<[_]>::to_vec);
            client.close().await;
            (values, peer.value)
        })
        .await
    });
    out
}

#[test]
fn the_late_answer_of_a_read_does_not_change_the_next_read() {
    for dropped in [0, 15] {
        let (values, value) = read_after_late_answer(dropped);
        assert_eq!(values, Ok(vec![Ok(value)]), "{dropped} dropped");
    }
}

#[test]
fn a_connect_with_no_listener_fails_with_its_status() {
    let (status, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let address = SocketAddr::new(node.addresses()[0], PORT + 1);
            peer.connect(&format!("opc.tcp://{address}")).await.err()
        })
        .await
    });
    assert_eq!(status, Some(Status::BAD_CONNECTION_REJECTED));
}

/// A port past `u32::MAX` that wraps to the port of the server fails the connect, and
/// does not reach the server.
#[test]
fn a_connect_to_a_port_that_wraps_to_the_server_fails_with_its_status() {
    let (status, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let wrapped = u64::from(PORT) + (1 << 32);
            let url = format!("opc.tcp://{}:{wrapped}", node.addresses()[0]);
            match peer.connect(&url).await {
                Ok(client) => {
                    client.close().await;
                    None
                }
                Err(status) => Some(status),
            }
        })
        .await
    });
    assert_eq!(status, Some(Status::BAD_TCP_ENDPOINT_URL_INVALID));
}

/// `UA_readNumber` stops before a digit that takes the number past `u32::MAX`.
#[test]
fn a_number_stops_before_the_digit_that_overflows_u32() {
    let read = |text: &str| {
        let mut number = 0;
        // SAFETY: `text` lives through the call, which reads only its `len` bytes.
        let progress = unsafe {
            ffi::test::UA_readNumber(text.as_ptr(), text.len(), &raw mut number)
        };
        (progress, number)
    };
    assert_eq!(read("4294967295"), (10, u32::MAX));
    assert_eq!(read("4294967296"), (9, 429_496_729));
    assert_eq!(read("42949672950"), (10, u32::MAX));
    assert_eq!(read("4840/path"), (4, 4840));
}

#[test]
fn a_connect_to_a_url_that_open62541_refuses_fails_with_its_status() {
    let (statuses, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut statuses = Vec::new();
            for url in ["http://127.0.0.1:4840", "opc.tcp://plc:99999"] {
                statuses.push(peer.connect(url).await.err());
            }
            statuses
        })
        .await
    });
    assert_eq!(
        statuses,
        [
            Some(Status::BAD_TCP_ENDPOINT_URL_INVALID),
            Some(Status::BAD_TCP_ENDPOINT_URL_INVALID)
        ]
    );
}

#[test]
fn a_read_after_the_server_closes_fails_and_open62541_refuses_the_next() {
    let (reads, _) = run(0, async |node| {
        let mut client = serve(&node, 4, async |mut peer| peer.connected().await).await;
        (client.read_one(&VALUE).await, client.read_one(&VALUE).await)
    });
    assert_eq!(
        reads,
        (
            Err(Status::BAD_SECURE_CHANNEL_CLOSED),
            Err(Status::BAD_SERVER_NOT_CONNECTED)
        )
    );
}

/// The server answers a read whose future dropped before the next read, and the
/// callback drops that answer, so the next read gets its own answer.
#[test]
fn a_read_after_a_dropped_read_gets_its_own_answer() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            {
                let mut dropped = pin!(client.read_one(&VALUE));
                poll_fn(|cx| {
                    assert!(dropped.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            peer.clock.sleep(Span::from_nanos(10_000_000)).await;
            (
                client.read_one(&TIME).await,
                client.read_one(&VALUE).await,
                peer.value,
            )
        })
        .await
    });
    let (time, value, expected) = reads;
    assert_eq!(
        (time, value),
        (Err(Status::BAD_TYPE_MISMATCH), Ok(expected))
    );
}

/// Starts `count` reads of `client`, each polled once and then dropped.
async fn drop_reads(client: &mut Client, count: u16) {
    for _ in 0..count {
        let mut read = pin!(client.read_one(&VALUE));
        poll_fn(|cx| {
            assert!(read.as_mut().poll(cx).is_pending(), "a read waits");
            Poll::Ready(())
        })
        .await;
    }
}

/// Without the bound on the reads in flight, the server answers the 257 requests of
/// the reads and the connect in one pass, and its 257th send closes the channel.
#[test]
fn a_read_after_255_dropped_reads_gives_the_value() {
    let ((read, value), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            drop_reads(&mut client, 255).await;
            let read = client.read_one(&VALUE).await;
            client.close().await;
            (read, peer.value)
        })
        .await
    });
    assert_eq!(read, Ok(value));
}

/// While the server stops, a read after `dropped` dropped reads starts 2 s after them.
/// Gives its answer and the time it took.
fn read_after(dropped: u16) -> (Result<f64, Status>, Span) {
    let (out, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.paused.set(true);
            drop_reads(&mut client, dropped).await;
            peer.clock.sleep(Span::from_nanos(2_000_000_000)).await;
            let start = peer.clock.now();
            let read = client.read_one(&VALUE).await;
            let took = peer.clock.now() - start;
            peer.paused.set(false);
            client.close().await;
            (read, took)
        })
        .await
    });
    out
}

/// Below the bound, a read sends its request and times out. At the bound, the dropped
/// reads stay in flight, so it fails at the end of its wait and sends nothing.
#[test]
fn a_read_at_the_bound_fails_when_no_read_ends() {
    let (below, below_took) = read_after(15);
    let (at, at_took) = read_after(16);
    assert_eq!(
        (below, at),
        (
            Err(Status::BAD_TIMEOUT),
            Err(Status::BAD_TOO_MANY_OPERATIONS)
        )
    );
    let timeout = Span::from_nanos(5_000_000_000)..=Span::from_nanos(6_000_000_000);
    assert!(timeout.contains(&below_took), "{below_took:?}");
    assert!(timeout.contains(&at_took), "{at_took:?}");
}

/// Reads [`VALUE`] while [`Peer::paused`] is set, except for one pass of the server
/// `at` after the read starts. Gives its answer and the time it took.
async fn read_with_pass(
    client: &mut Client,
    peer: &Peer<'_>,
    at: Span,
) -> (Result<f64, Status>, Span) {
    let start = peer.clock.now();
    let mut pass = pin!(peer.clock.sleep(at));
    let (mut woke, mut passed) = (false, false);
    let mut read = pin!(client.read_one(&VALUE));
    let read = poll_fn(|cx| {
        if !woke && pass.as_mut().poll(cx).is_ready() {
            woke = true;
            peer.paused.set(false);
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        if woke && !passed {
            passed = true;
            peer.paused.set(true);
        }
        read.as_mut().poll(cx)
    })
    .await;
    (read, peer.clock.now() - start)
}

/// A read at the bound whose wait ends 3 s in still times out 5 s after it starts.
#[test]
fn the_timeout_of_a_read_counts_its_wait() {
    let ((read, took), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.paused.set(true);
            drop_reads(&mut client, 16).await;
            let out =
                read_with_pass(&mut client, &peer, Span::from_nanos(3_000_000_000))
                    .await;
            peer.paused.set(false);
            client.close().await;
            out
        })
        .await
    });
    assert_eq!(read, Err(Status::BAD_TIMEOUT));
    let timeout = Span::from_nanos(5_000_000_000)..=Span::from_nanos(6_000_000_000);
    assert!(timeout.contains(&took), "{took:?}");
}

/// The answers of 16 dropped reads end them all, so a read after one more dropped read
/// sends its request at once and gets its answer in one pass of the server.
#[test]
fn the_answers_of_dropped_reads_end_them() {
    let ((read, value), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            drop_reads(&mut client, 16).await;
            peer.clock.sleep(Span::from_nanos(1_000_000_000)).await;
            peer.paused.set(true);
            drop_reads(&mut client, 1).await;
            let (read, _) =
                read_with_pass(&mut client, &peer, Span::from_nanos(1_000_000_000))
                    .await;
            peer.paused.set(false);
            client.close().await;
            (read, peer.value)
        })
        .await
    });
    assert_eq!(read, Ok(value));
}

/// A stalled server keeps the request of each read that timed out, and answers them
/// all in one pass when it runs again. Without the bound on its requests, 17 rounds of
/// 16 dropped reads send 272, and the 257th answer closes the channel.
#[test]
fn a_read_after_272_dropped_reads_on_a_stalled_server_gives_the_value() {
    let ((read, value), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.paused.set(true);
            for _ in 0..17 {
                drop_reads(&mut client, 16).await;
                peer.clock.sleep(Span::from_nanos(6_000_000_000)).await;
            }
            peer.paused.set(false);
            peer.clock.sleep(Span::from_nanos(10_000_000)).await;
            let read = client.read_one(&VALUE).await;
            client.close().await;
            (read, peer.value)
        })
        .await
    });
    assert_eq!(read, Ok(value));
}

/// Reads [`FITS`] and then [`PAST`] from a server that sends at most `chunks` chunks in
/// a message.
fn read_large(chunks: u32) -> (Result<f64, Status>, Result<f64, Status>) {
    let (reads, _) = run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.add_bytes(&FITS, 7 * CHUNK);
            peer.server.add_bytes(&PAST, 8 * CHUNK);
            peer.server.set_limits(Limits {
                chunks,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            let reads = (client.read_one(&FITS).await, client.read_one(&PAST).await);
            client.close().await;
            reads
        })
        .await
    });
    reads
}

/// The answer of [`FITS`] arrives and is not a Double. The answer of [`PAST`] needs
/// more than [`CHUNKS`] chunks. The test server refuses to send it, and sends an
/// error that closes the channel. A server that sends it makes the client close the
/// channel at the chunk past the bound.
#[test]
fn an_answer_of_more_chunks_than_the_bound_fails() {
    let large = (
        Err(Status::BAD_TYPE_MISMATCH),
        Err(Status::BAD_SECURE_CHANNEL_CLOSED),
    );
    assert_eq!(read_large(CHUNKS), large);
    assert_eq!(read_large(0), large);
}

/// A drop of a client with an active session ends, and calls the callback of each
/// read in flight, so the answer of a client must outlive its delete. The server then
/// closes. The test reads through C on the private client, since `read` borrows the
/// client, so no public call keeps a read in flight across a drop.
#[test]
fn a_drop_of_a_connected_client_ends_and_answers_each_read() {
    let (value, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let client = peer.connected().await;
            let outside = Reads::default();
            let nodes = Nodes::new(&[VALUE], Rc::default());
            let mut request = 0;
            // SAFETY: the client and `nodes` live, and `outside` outlives the client.
            let status = Status(unsafe {
                ffi::shim_client_read(
                    client.raw.as_ptr(),
                    nodes.raw(),
                    answer,
                    std::ptr::from_ref(&outside).cast_mut().cast(),
                    &raw mut request,
                )
            });
            assert_eq!(status, Status::GOOD);
            outside.pending.set(1);
            outside.request.set(Some(request));
            drop(client);
            outside.answer.take()
        })
        .await
    });
    assert_eq!(value, Some(Status::BAD_SESSION_CLOSED));
}

/// A server with room for one session takes a second client only after the first
/// closes its session.
#[test]
fn a_close_frees_the_session_on_the_server_and_a_drop_does_not() {
    let (connects, _) = run(0, async |node| {
        serve(&node, 1, async |mut peer| {
            peer.connected().await.close().await;
            drop(peer.connected().await);
            let url = peer.url.clone();
            peer.connect(&url).await.err()
        })
        .await
    });
    assert_eq!(connects, Some(Status::BAD_TOO_MANY_SESSIONS));
}

/// Connects to a listener on `node` that gives `accepted` each stream it accepts, and
/// gives the status of the failed connect and the time it took.
async fn connect_to_listener(
    node: &node::Node,
    mut accepted: impl FnMut(Tcp),
) -> (Status, Span) {
    let local = SocketAddr::new(node.addresses()[0], PORT);
    let listen = tcp::Listen {
        local,
        backlog: 4,
        options: options(),
    };
    let mut listener = node.net().listen(&listen).expect("the port is free");
    let clock = node.clock();
    let start = clock.now();
    let mut rng = node.entropy().rng();
    let url = CString::new(format!("opc.tcp://{local}")).expect("no NUL");
    let mut connect = pin!(Client::connect(
        node.clock(),
        event::monotonic(node.clock()),
        node.net(),
        &mut rng,
        &url
    ));
    let status = poll_fn(|cx| {
        while let Poll::Ready(stream) = listener.poll_accept(cx) {
            accepted(stream.expect("the accept works"));
        }
        connect.as_mut().poll(cx).map(Result::err)
    })
    .await;
    (status.expect("the connect fails"), clock.now() - start)
}

/// The options of a stream of a test.
fn options() -> tcp::Options {
    tcp::Options {
        send_buffer_bytes: 1 << 16,
        recv_buffer_bytes: 1 << 16,
        unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
        delayed: false,
    }
}

#[test]
fn a_connect_to_a_listener_that_never_answers_times_out() {
    let mut streams = Vec::new();
    let (out, _) = run(0, async move |node| {
        connect_to_listener(&node, |stream| streams.push(stream)).await
    });
    assert_eq!(out, (Status::BAD_TIMEOUT, Span::from_nanos(5_000_000_000)));
}

#[test]
fn a_connect_to_a_listener_that_closes_each_stream_times_out() {
    let (out, _) = run(0, async |node| connect_to_listener(&node, drop).await);
    assert_eq!(out, (Status::BAD_TIMEOUT, Span::from_nanos(5_000_000_000)));
}

/// A close waits for the answer of the server, which a server that stops never gives,
/// until the request times out after 5 s. open62541 checks the timeouts of requests
/// once a second.
#[test]
fn a_close_of_a_client_whose_server_stops_ends_at_the_request_timeout() {
    let (took, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let client = peer.connected().await;
            peer.paused.set(true);
            let start = peer.clock.now();
            client.close().await;
            peer.paused.set(false);
            peer.clock.now() - start
        })
        .await
    });
    let window = Span::from_nanos(5_000_000_000)..=Span::from_nanos(6_000_000_000);
    assert!(window.contains(&took), "{took:?}");
}

/// A read restores the request timeout of the client after it sends, so a close after a
/// read on a server that stops still ends 5 to 6 s after it starts.
#[test]
fn a_close_after_a_read_of_a_client_whose_server_stops_ends_at_the_request_timeout() {
    let (took, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
            peer.paused.set(true);
            let start = peer.clock.now();
            client.close().await;
            peer.paused.set(false);
            peer.clock.now() - start
        })
        .await
    });
    let window = Span::from_nanos(5_000_000_000)..=Span::from_nanos(6_000_000_000);
    assert!(window.contains(&took), "{took:?}");
}

/// A read that a server that stops never answers ends with `BadTimeout` at the request
/// timeout, as a close does.
#[test]
fn a_read_of_a_client_whose_server_stops_times_out() {
    let ((value, took), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            peer.paused.set(true);
            let start = peer.clock.now();
            let value = client.read_one(&VALUE).await;
            let took = peer.clock.now() - start;
            peer.paused.set(false);
            client.close().await;
            (value, took)
        })
        .await
    });
    assert_eq!(value, Err(Status::BAD_TIMEOUT));
    let window = Span::from_nanos(5_000_000_000)..=Span::from_nanos(6_000_000_000);
    assert!(window.contains(&took), "{took:?}");
}

/// A connect that times out after the server makes its session closes the session, so
/// a server with room for one session takes the next client. In the run of seed 0, the
/// server makes the session 3 ms after the connect starts and activates it later.
#[test]
fn a_connect_that_times_out_closes_its_session() {
    times_out_and_closes(&[Span::from_nanos(3_000_000)]);
}

/// A connect that times out before the server answers its `CreateSession` closes the
/// session that the server makes after the deadline. In the run of seed 0, the server
/// gets the `CreateSession` before 2.5 ms and answers it 100 us after the deadline.
#[test]
fn a_connect_that_times_out_before_the_server_makes_its_session_closes_it() {
    times_out_and_closes(&[
        Span::from_nanos(2_500_000),
        Span::from_nanos(5_000_100_000),
    ]);
}

/// A connect that times out before a server that stops answers its `CreateSession`
/// ends once that request times out, and leaves no session.
#[test]
fn a_connect_whose_create_session_gets_no_answer_ends() {
    times_out_and_closes(&[Span::from_nanos(2_500_000)]);
}

/// The close after a connect that times out can take two more request timeouts: one for
/// the answer to the `CreateSession`, then one for the `CloseSession`. The server gets
/// the `CreateSession` just before the deadline and answers it at 9 s, or gets it just
/// after the check of timeouts at 4 s and answers it just before its timeout, and then
/// stops. open62541 checks the timeouts of requests once a second from the start of the
/// connect, so the close ends at most 10 s after the deadline.
#[test]
fn a_connect_whose_create_session_answers_late_ends_two_request_timeouts_later() {
    let before_the_deadline = times_out_and_closes(&[
        Span::from_nanos(0),
        Span::from_nanos(4_998_000_000),
        Span::from_nanos(4_999_500_000),
        Span::from_nanos(9_000_000_000),
        Span::from_nanos(9_000_100_000),
    ]);
    let after_a_check = times_out_and_closes(&[
        Span::from_nanos(0),
        Span::from_nanos(4_001_000_000),
        Span::from_nanos(4_002_500_000),
        Span::from_nanos(9_999_000_000),
        Span::from_nanos(9_999_100_000),
    ]);
    let bound = Span::from_nanos(15_000_000_000);
    assert_eq!((before_the_deadline, after_a_check), (bound, bound));
}

/// Connects to a server with room for one session that stops at the first time of
/// `flips` after the connect starts, runs again at the next, and so on, and runs from
/// the end of the connect. Asserts that the connect fails with `BadTimeout` and that
/// a second client then connects, so the first left no session. Gives the time that
/// the first connect took.
fn times_out_and_closes(flips: &[Span]) -> Span {
    let flips = flips.to_vec();
    let ((first, took, second), _) = run(0, async move |node| {
        serve(&node, 1, async |mut peer| {
            let (url, paused) = (peer.url.clone(), Rc::clone(&peer.paused));
            let raw = CString::new(url.as_str()).expect("no NUL");
            let (clock, net) = (Clock::clone(&peer.clock), Net::clone(&peer.net));
            let wall = event::monotonic(Clock::clone(&clock));
            let start = peer.clock.now();
            let first = {
                let mut connect =
                    pin!(Client::connect(clock, wall, net, &mut peer.rng, &raw));
                let mut flips: Vec<_> = flips
                    .iter()
                    .map(|at| Box::pin(peer.clock.sleep(*at)))
                    .collect();
                let mut flipped = 0;
                poll_fn(|cx| {
                    while flipped < flips.len()
                        && flips[flipped].as_mut().poll(cx).is_ready()
                    {
                        flipped += 1;
                        paused.set(!paused.get());
                        // `serve` runs the server before this body.
                        cx.waker().wake_by_ref();
                    }
                    connect.as_mut().poll(cx)
                })
                .await
                .err()
            };
            let took = peer.clock.now() - start;
            paused.set(false);
            let second = match peer.connect(&url).await {
                Ok(client) => {
                    client.close().await;
                    None
                }
                Err(status) => Some(status),
            };
            (first, took, second)
        })
        .await
    });
    assert_eq!((first, second), (Some(Status::BAD_TIMEOUT), None));
    took
}

/// A server adds a namespace after a client connects, and a Double in it. The client
/// reads it, then a new client reads it.
#[test]
fn a_read_of_a_namespace_that_the_server_adds_after_the_connect_gets_its_value() {
    let ((value, reads), _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let first = client.read_one(&VALUE).await;
            let namespace = peer.server.add_namespace(c"urn:added");
            let added = Node {
                namespace,
                key: Key::Number(1000),
            };
            peer.server.add_double(&added, 2.5);
            let second = client.read_one(&added).await;
            client.close().await;
            let mut fresh = peer.connected().await;
            let third = fresh.read_one(&added).await;
            fresh.close().await;
            (peer.value, (first, namespace, second, third))
        })
        .await
    });
    assert_eq!(reads, (Ok(value), 2, Ok(2.5), Ok(2.5)));
}

/// A node in namespace 1 named by `length` bytes.
fn long(length: usize) -> Node {
    let name = Name::new(&"x".repeat(length)).expect("a name that is not empty");
    Node {
        namespace: 1,
        key: Key::Name(name),
    }
}

/// Gives the length of the name of a [`long`] node after `nodes` whose request on
/// `client` is the most bytes of one message, from a name of `probe` bytes, whose
/// request is past it.
fn fitting(client: &Client, nodes: &[Node], probe: usize) -> usize {
    let mut nodes = nodes.to_vec();
    nodes.push(long(probe));
    match client.nodes(&nodes) {
        Err(Large { bytes, most }) => probe - (bytes - most),
        Ok(_) => panic!("a name of {probe} bytes fits"),
    }
}

/// Gives the most [`long`] nodes of `length` bytes that `client` takes in one request.
fn most_of(client: &Client, length: usize) -> usize {
    let (mut fits, mut past) = (1, 1 << 14);
    while past - fits > 1 {
        let count = usize::midpoint(fits, past);
        if client.nodes(&vec![long(length); count]).is_ok() {
            fits = count;
        } else {
            past = count;
        }
    }
    fits
}

/// The request of a session takes 8 chunks, [`CHUNKS`], of 65536 bytes less the
/// 24 bytes of their headers, less 7 bytes at the end of each chunk but the last. The
/// client refuses to send 1 byte more. A request whose one name fills each chunk to
/// its end is 49 bytes longer when open62541 refuses it.
#[test]
fn a_request_takes_the_chunks_of_one_message() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let length = fitting(&client, &[], 1 << 20);
            peer.server.add_double(&long(length), 2.5);
            let fits = client.nodes(&[long(length)]).expect("it fits");
            let read = client.read(&fits).await.map(<[_]>::to_vec);
            let past = client.nodes(&[long(length + 1)]).map(drop);
            let after = client.read_one(&VALUE).await;
            let unchecked = client.unchecked(&[long(length + 49)]);
            let full = client.read(&unchecked).await.map(<[_]>::to_vec);
            let unchecked = client.unchecked(&[long(length + 50)]);
            let sent = client.read(&unchecked).await.map(<[_]>::to_vec);
            client.close().await;
            (read, past, after, full, sent, peer.value)
        })
        .await
    });
    let (read, past, after, full, sent, value) = reads;
    assert_eq!(read, Ok(vec![Ok(2.5)]));
    assert_eq!(full, Ok(vec![Err(Status::BAD_NODE_ID_UNKNOWN)]));
    let most = 8 * (65_536 - 24) - 7 * 7;
    let large = Large {
        bytes: most + 1,
        most,
    };
    assert_eq!(past, Err(large));
    assert_eq!(after, Ok(value));
    assert_eq!(sent, Err(Status::BAD_RESPONSE_TOO_LARGE));
}

/// A server with a `MaxMessageSize` of 8192 bytes takes a request of 8192 bytes, and
/// refuses 1 byte more. A good answer of 239 Doubles takes at most 36 bytes and 34 per
/// node, 8162 bytes, and of 240 nodes, 8196 bytes.
#[test]
fn a_read_takes_the_max_message_size_of_the_server() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                message: 8192,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            let length = fitting(&client, &[], 9000);
            peer.server.add_double(&long(length), 2.5);
            let fits = client.nodes(&[long(length)]).expect("it fits");
            let read = client.read(&fits).await.map(<[_]>::to_vec);
            let past = client.nodes(&[long(length + 1)]).map(drop);
            let most = client.nodes(&[VALUE; 239]).expect("239 nodes fit");
            let values = client.read(&most).await.map(<[_]>::to_vec);
            let answer_past = client.nodes(&[VALUE; 240]).map(drop);
            let unchecked = client.unchecked(&[long(length + 1)]);
            let sent = client.read(&unchecked).await.map(<[_]>::to_vec);
            client.close().await;
            (read, past, values, answer_past, sent, peer.value)
        })
        .await
    });
    let (read, past, values, answer_past, sent, value) = reads;
    assert_eq!(read, Ok(vec![Ok(2.5)]));
    let large = |bytes| Large { bytes, most: 8192 };
    assert_eq!(past, Err(large(8193)));
    assert_eq!(values, Ok(vec![Ok(value); 239]));
    assert_eq!(answer_past, Err(large(8196)));
    assert_eq!(sent, Err(Status::BAD_SECURE_CHANNEL_CLOSED));
}

/// A server with buffers of 8192 bytes and a `MaxChunkCount` of 4 gives a message of 4
/// chunks of 8168 bytes less 21: 959 nodes, and not 960.
#[test]
fn a_read_takes_the_buffers_and_chunks_of_the_server() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_limits(Limits {
                buffer: 8192,
                chunks: 4,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            let most = client.nodes(&vec![VALUE; 959]).expect("959 nodes fit");
            let values = client.read(&most).await.map(<[_]>::to_vec);
            let past = client.nodes(&vec![VALUE; 960]).map(drop);
            client.close().await;
            (values, past, peer.value)
        })
        .await
    });
    let (values, past, value) = reads;
    assert_eq!(values, Ok(vec![Ok(value); 959]));
    let large = Large {
        bytes: 36 + 960 * 34,
        most: 4 * 8168 - 3 * 7,
    };
    assert_eq!(past, Err(large));
}

/// A client with a receive buffer of 8192 bytes sends chunks of 65536 bytes and takes
/// chunks of 8192 bytes: a request takes 8 chunks of 65512 bytes less 49, and a good
/// answer 8 chunks of 8168 bytes less 49, 1919 Doubles and not 1920.
#[test]
fn a_request_and_an_answer_each_take_the_buffer_of_their_side() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.receiving(8192).await;
            let length = fitting(&client, &[], 1 << 20);
            let request = client.nodes(&[long(length + 1)]).map(drop);
            let most = client.nodes(&vec![VALUE; 1919]).expect("1919 nodes fit");
            let values = client.read(&most).await.map(<[_]>::to_vec);
            let answer = client.nodes(&vec![VALUE; 1920]).map(drop);
            client.close().await;
            (request, values, answer, peer.value)
        })
        .await
    });
    let (request, values, answer, value) = reads;
    let most = 8 * (65_536 - 24) - 7 * 7;
    let large = Large {
        bytes: most + 1,
        most,
    };
    assert_eq!(request, Err(large));
    assert_eq!(values, Ok(vec![Ok(value); 1919]));
    let large = Large {
        bytes: 36 + 1920 * 34,
        most: 8 * 8168 - 7 * 7,
    };
    assert_eq!(answer, Err(large));
}

/// The limits of the channel live only in the copy that the key gives.
#[test]
fn the_key_of_the_limits_gives_no_value_without_a_copy() {
    let (status, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let client = peer.connected().await;
            let mut value = vec![0_u64; ffi::types()[ffi::VARIANT].size().div_ceil(8)];
            let key = ffi::test::QualifiedName {
                namespace: 0,
                name: ffi::Bytes {
                    length: 13,
                    data: b"messageLimits".as_ptr().cast_mut(),
                },
            };
            // SAFETY: the client lives, `value` has room for a `UA_Variant`, and the
            // key outlives the call.
            let status = Status(unsafe {
                ffi::test::UA_Client_getConnectionAttribute(
                    client.raw.as_ptr(),
                    key,
                    value.as_mut_ptr().cast(),
                )
            });
            client.close().await;
            status
        })
        .await
    });
    assert_eq!(status, Status::BAD_NOT_SUPPORTED);
}

#[test]
fn a_limit_of_0_from_a_server_is_no_limit() {
    assert_eq!(most(8192, 0, 0), 8 * 8168 - 7 * 7);
    assert_eq!(most(8192, 0, 9), 8 * 8168 - 7 * 7);
    assert_eq!(most(8192, 0, 7), 7 * 8168 - 6 * 7);
    assert_eq!(most(8192, 50_000, 0), 50_000);
}

/// No form of a key leaves more than [`SPLIT`] bytes empty at a chunk end. A new form
/// does not build here until it gives the largest part of it that open62541 moves
/// whole, and that part does not build past `SPLIT + 1`.
#[test]
fn no_form_of_a_key_leaves_more_than_split() {
    // The match and its `const` blocks are the test: each runs when the test builds,
    // with no call.
    let _ = |key: &Key| match key {
        // The mask, the namespace, and the number of a NodeId by number.
        Key::Number(_) => const { assert!(7 - 1 <= SPLIT) },
        // The length of the string of a NodeId by name: its bytes split.
        Key::Name(_) => const { assert!(4 - 1 <= SPLIT) },
    };
}

/// The most nodes that one request takes are read, also when open62541 leaves the end
/// of a chunk empty: names of 37 and 44 bytes in chunks of 65536 bytes, and 450 to 469
/// numbers and a name that fills the request in chunks of 8192 bytes.
#[test]
fn the_most_nodes_of_one_request_are_read() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let mut reads = Vec::new();
            for length in [37, 44] {
                let count = most_of(&client, length);
                let nodes = client.nodes(&vec![long(length); count]).expect("it fits");
                reads.push((count, client.read(&nodes).await.map(<[_]>::len)));
            }
            client.close().await;
            peer.server.set_limits(Limits {
                buffer: 8192,
                chunks: 4,
                ..Limits::default()
            });
            let mut client = peer.connected().await;
            for count in 450..470 {
                let mut list = vec![VALUE; count];
                list.push(long(fitting(&client, &list, 1 << 15)));
                let nodes = client.nodes(&list).expect("it fits");
                reads.push((count + 1, client.read(&nodes).await.map(<[_]>::len)));
            }
            client.close().await;
            reads
        })
        .await
    });
    for (count, read) in reads {
        assert_eq!(read, Ok(count));
    }
}

/// A read of no nodes gives no values, and the next read its value.
#[test]
fn a_read_of_no_nodes_gives_no_values() {
    let (reads, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            let none = client.nodes(&[]).expect("no nodes fit");
            let read = client.read(&none).await.map(<[_]>::to_vec);
            let after = client.read_one(&VALUE).await;
            client.close().await;
            (read, after, peer.value)
        })
        .await
    });
    let (read, after, value) = reads;
    assert_eq!(read, Ok(vec![]));
    assert_eq!(after, Ok(value));
}

/// Reads the list of `nodes` that one client checked on another client, and gives the
/// text of the panic. The panic is caught inside `serve`, so the server closes.
fn read_on_another_client(nodes: &'static [Node]) -> Option<&'static str> {
    run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let first = peer.connected().await;
            let mut second = peer.connected().await;
            let nodes = first.nodes(nodes).expect("the nodes fit");
            let panic = caught(second.read(&nodes))
                .await
                .expect_err("the read panics");
            panic.downcast_ref::<&str>().copied()
        })
        .await
    })
    .0
}

/// A client reads only a list that it checked, as the limits of another session can
/// be smaller.
#[test]
fn a_read_of_the_nodes_of_another_client_panics() {
    assert_eq!(
        read_on_another_client(&[VALUE]),
        Some("invariant: a client reads only the nodes that it checked")
    );
}

/// A client reads only a list that it checked, also when the list is empty.
#[test]
fn a_read_of_no_nodes_of_another_client_panics() {
    assert_eq!(
        read_on_another_client(&[]),
        Some("invariant: a client reads only the nodes that it checked")
    );
}

/// The port of the relay of [`relayed`].
const RELAY: u16 = 4841;

/// The binary encoding of a Read response, a `NodeId` of 4 bytes.
const READ_RESPONSE: [u8; 4] = [0x01, 0x00, 0x7A, 0x02];

/// Runs `body` while a relay on `node` moves the bytes of the first stream to
/// [`RELAY`] to the test server and back, and gives each message of the server to
/// `rewrite` before the client gets it.
async fn relayed<T>(
    node: &node::Node,
    rewrite: impl FnMut(&mut Vec<u8>),
    body: impl Future<Output = T>,
) -> T {
    let mut relay = pin!(relay(node, rewrite));
    let mut body = pin!(body);
    poll_fn(|cx| {
        assert!(relay.as_mut().poll(cx).is_pending(), "it never ends");
        body.as_mut().poll(cx)
    })
    .await
}

async fn relay(node: &node::Node, mut rewrite: impl FnMut(&mut Vec<u8>)) {
    let address = node.addresses()[0];
    let listen = tcp::Listen {
        local: SocketAddr::new(address, RELAY),
        backlog: 4,
        options: options(),
    };
    let mut listener = node.net().listen(&listen).expect("the port is free");
    let mut client = poll_fn(|cx| listener.poll_accept(cx))
        .await
        .expect("the accept works");
    let config = tcp::Config {
        remote: SocketAddr::new(address, PORT),
        options: options(),
    };
    let mut server = node
        .net()
        .connect(&config)
        .await
        .expect("the server listens");
    let (mut up, mut down) = (Pipe::default(), Pipe::default());
    poll_fn(|cx| {
        up.pump(cx, &mut client, &mut server, |_| {});
        down.pump(cx, &mut server, &mut client, &mut rewrite);
        Poll::<()>::Pending
    })
    .await;
}

/// One direction of a relay.
#[derive(Default)]
struct Pipe {
    /// The bytes read after the last whole message.
    read: Vec<u8>,
    /// The bytes not yet written.
    write: Vec<u8>,
}

impl Pipe {
    /// Moves the bytes that `from` has to `to`, and gives each whole message to
    /// `rewrite` first.
    fn pump(
        &mut self,
        cx: &mut Context<'_>,
        from: &mut Tcp,
        to: &mut Tcp,
        mut rewrite: impl FnMut(&mut Vec<u8>),
    ) {
        let mut buffer = [0; 1 << 12];
        while let Poll::Ready(read) = from.poll_read(cx, &mut buffer) {
            let read = read.expect("the read works");
            if read == 0 {
                break;
            }
            self.read.extend_from_slice(&buffer[..read]);
        }
        while let Some(size) = self.read.get(4..8) {
            let size = u32::from_le_bytes(size.try_into().expect("4 bytes"));
            let size = usize::try_from(size).expect("a usize holds a u32");
            if self.read.len() < size {
                break;
            }
            let mut message = self.read.drain(..size).collect();
            rewrite(&mut message);
            self.write.extend(message);
        }
        while !self.write.is_empty() {
            let Poll::Ready(written) = to.poll_write(cx, &[IoSlice::new(&self.write)])
            else {
                break;
            };
            self.write.drain(..written.expect("the write works"));
        }
    }
}

/// Gives the request handle of `message` when it is a Read response.
fn handle(message: &[u8]) -> Option<u32> {
    let read = message.starts_with(b"MSG") && message[24..28] == READ_RESPONSE;
    read.then(|| u32::from_le_bytes(message[36..40].try_into().expect("4 bytes")))
}

/// Makes the Read response `message`, with no diagnostics, string table, or header
/// extension, give `count` empty results.
fn recount_of(message: &mut Vec<u8>, count: i32) {
    assert_eq!(message[44..52], [0, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0]);
    let diagnostics = message.len() - 4;
    assert_eq!(message[diagnostics..], [0xFF; 4]);
    let results =
        count.to_le_bytes().into_iter().chain(vec![
            0;
            usize::try_from(count)
                .expect("a count of 0 or more")
        ]);
    message.splice(52..diagnostics, results);
    let size = u32::try_from(message.len()).expect("a message of a u32 of bytes");
    message[4..8].copy_from_slice(&size.to_le_bytes());
}

/// The values of a read, or its error.
type Read = Result<Vec<Result<f64, Status>>, Status>;

/// Gives two reads of [`VALUE`] by one client through a relay that gives the first
/// Read response `recount` empty results when it is set, the request handle of each
/// Read response, the value of [`VALUE`], and the count of values that the first
/// answer copies into the client.
fn read_twice(recount: Option<i32>) -> ([Read; 2], Vec<u32>, f64, usize) {
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let mut handles = Vec::new();
            let rewrite = |message: &mut Vec<u8>| {
                if let Some(handle) = handle(message) {
                    if let (true, Some(count)) = (handles.is_empty(), recount) {
                        recount_of(message, count);
                    }
                    handles.push(handle);
                }
            };
            let (reads, copied) = relayed(&node, rewrite, async {
                let mut client = peer.connect(&url).await.expect("the client connects");
                let nodes = client.nodes(&[VALUE]).expect("one node fits");
                let first = client.read(&nodes).await.map(<[_]>::to_vec);
                let values = client.reads.values.take();
                let copied = values.len();
                client.reads.values.set(values);
                let second = client.read(&nodes).await.map(<[_]>::to_vec);
                client.close().await;
                ([first, second], copied)
            })
            .await;
            (reads, handles, peer.value, copied)
        })
        .await
    })
    .0
}

/// Each read of one request has its own request handle on the wire.
#[test]
fn each_read_of_one_request_has_its_own_handle() {
    let (reads, handles, value, _) = read_twice(None);
    assert_eq!(reads, [Ok(vec![Ok(value)]), Ok(vec![Ok(value)])]);
    let [one, two] = handles[..] else {
        panic!("two Read responses, not {}", handles.len());
    };
    assert_ne!(one, two);
}

/// An answer with a value for more or fewer nodes than its read fails, copies none of
/// them, and the next read gives the value of each node. The test reads the buffer of
/// the client: no call gives its size, and the next good answer clears it before it
/// copies, so no caller sees a copied value.
#[test]
fn an_answer_of_another_count_of_values_fails() {
    for count in [0, 2] {
        let (reads, _, value, copied) = read_twice(Some(count));
        assert_eq!(
            (reads, copied),
            ([Err(Status::BAD_UNEXPECTED_ERROR), Ok(vec![Ok(value)])], 0),
            "{count} results"
        );
    }
}
