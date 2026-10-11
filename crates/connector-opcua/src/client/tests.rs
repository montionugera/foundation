use std::cell::Cell;
use std::future::{Future as _, poll_fn};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

use env::clock::Clock;
use env::net::{Net, Tcp, tcp};
use env::rng::Rng;
use sim::{Sim, node};
use types::time::Span;

use super::{Answer, Client, Node, answer};
use crate::connection::Manager;
use crate::ffi::{self, Status};
use crate::server::Server;

/// The port of the test server.
const PORT: u16 = 4840;

/// The Double variable of the test server.
const VALUE: Node = Node {
    namespace: 1,
    key: 1000,
};

/// The current time of the server, a `DateTime`.
const TIME: Node = Node {
    namespace: 0,
    key: ffi::test::TIME,
};

/// The side of a client in a run of [`serve`].
struct Peer {
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

impl Peer {
    async fn connect(&mut self, url: &str) -> Result<Client, Status> {
        let (clock, net) = (Clock::clone(&self.clock), Net::clone(&self.net));
        Client::connect(clock, net, &mut self.rng, url).await
    }

    async fn connected(&mut self) -> Client {
        let url = self.url.clone();
        self.connect(&url).await.expect("the client connects")
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
    body: impl AsyncFnOnce(Peer) -> T,
) -> T {
    let mut rng = node.entropy().rng();
    let local = SocketAddr::new(node.addresses()[0], PORT);
    let manager = Manager::listening(node.clock(), node.net(), local, 4, &mut rng)
        .expect("the port is free");
    // SAFETY: the close below deletes the server before the manager drops.
    let server = unsafe { Server::start(&manager, c"", sessions) };
    let value = f64::from(u32::try_from(rng.below(1 << 20)).expect("fits"));
    server.add_double(VALUE, value);
    let peer = Peer {
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
            (peer.value, client.read(VALUE).await)
        })
        .await
    })
}

#[test]
fn a_client_reads_a_double_and_a_seed_replays() {
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
            let unknown = Node { key: 1001, ..VALUE };
            client.read(unknown).await
        })
        .await
    });
    assert_eq!(read, Err(Status::BAD_NODE_ID_UNKNOWN));
}

#[test]
fn a_read_of_another_type_fails_with_a_type_mismatch() {
    let (read, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut client = peer.connected().await;
            client.read(TIME).await
        })
        .await
    });
    assert_eq!(read, Err(Status::BAD_TYPE_MISMATCH));
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

#[test]
fn a_connect_to_a_url_that_open62541_refuses_fails_with_its_status() {
    let (statuses, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let mut statuses = Vec::new();
            for url in ["http://127.0.0.1:4840", "opc.tcp://a\0b:4840"] {
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
        (client.read(VALUE).await, client.read(VALUE).await)
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
                let mut dropped = pin!(client.read(VALUE));
                poll_fn(|cx| {
                    assert!(dropped.as_mut().poll(cx).is_pending());
                    Poll::Ready(())
                })
                .await;
            }
            peer.clock.sleep(Span::from_nanos(10_000_000)).await;
            (
                client.read(TIME).await,
                client.read(VALUE).await,
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

/// A drop of a client with an active session ends, and calls the callback of each
/// read in flight, so the answer of a client must outlive its delete. The server then
/// closes. The test reads through C on the private client, since `read` borrows the
/// client, so no public call keeps a read in flight across a drop.
#[test]
fn a_drop_of_a_connected_client_ends_and_answers_each_read() {
    let (value, _) = run(0, async |node| {
        serve(&node, 4, async |mut peer| {
            let client = peer.connected().await;
            let outside = Answer::default();
            let mut request = 0;
            // SAFETY: the client lives, and `outside` outlives it.
            let status = Status(unsafe {
                ffi::shim_client_read(
                    client.raw.as_ptr(),
                    VALUE.namespace,
                    VALUE.key,
                    answer,
                    std::ptr::from_ref(&outside).cast_mut().cast(),
                    &raw mut request,
                )
            });
            assert_eq!(status, Status::GOOD);
            outside.request.set(Some(request));
            drop(client);
            outside.value.take()
        })
        .await
    });
    assert_eq!(value, Some(Err(Status::BAD_SESSION_CLOSED)));
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
        options: tcp::Options {
            send_buffer_bytes: 1 << 16,
            recv_buffer_bytes: 1 << 16,
            unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
            delayed: false,
        },
    };
    let mut listener = node.net().listen(&listen).expect("the port is free");
    let clock = node.clock();
    let start = clock.now();
    let mut rng = node.entropy().rng();
    let url = format!("opc.tcp://{local}");
    let mut connect = pin!(Client::connect(node.clock(), node.net(), &mut rng, &url));
    let status = poll_fn(|cx| {
        while let Poll::Ready(stream) = listener.poll_accept(cx) {
            accepted(stream.expect("the accept works"));
        }
        connect.as_mut().poll(cx).map(Result::err)
    })
    .await;
    (status.expect("the connect fails"), clock.now() - start)
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

/// A connect that times out after the server makes its session closes the session, so
/// a server with room for one session takes the next client. In the run of seed 0, the
/// server makes the session 3 ms after the connect starts and activates it later.
#[test]
fn a_connect_that_times_out_closes_its_session() {
    let (connects, _) = run(0, async |node| {
        serve(&node, 1, async |mut peer| {
            let (url, paused) = (peer.url.clone(), Rc::clone(&peer.paused));
            let (clock, net) = (Clock::clone(&peer.clock), Net::clone(&peer.net));
            let first = {
                let mut connect =
                    pin!(Client::connect(clock, net, &mut peer.rng, &url));
                let mut made = pin!(peer.clock.sleep(Span::from_nanos(3_000_000)));
                poll_fn(|cx| {
                    if !paused.get() && made.as_mut().poll(cx).is_ready() {
                        paused.set(true);
                    }
                    connect.as_mut().poll(cx)
                })
                .await
                .err()
            };
            paused.set(false);
            let second = match peer.connect(&url).await {
                Ok(client) => {
                    client.close().await;
                    None
                }
                Err(status) => Some(status),
            };
            (first, second)
        })
        .await
    });
    assert_eq!(connects, (Some(Status::BAD_TIMEOUT), None));
}
