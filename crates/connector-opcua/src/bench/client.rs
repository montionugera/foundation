//! Runs a test server of open62541 with clients that each have their own connection
//! manager, as the kind of OPC UA uses them.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::RefCell;
use std::future::{Future as _, poll_fn};
use std::mem::ManuallyDrop;
use std::net::{IpAddr, SocketAddr};
use std::panic;
use std::pin::pin;
use std::task::Poll;

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;

use super::PORT;
use crate::client::Node;
use crate::connection;

/// The Double variable of a [`Server`].
const VALUE: Node = Node {
    namespace: 1,
    key: 1000,
};
/// The value of [`VALUE`].
const DOUBLE: f64 = 1.5;
/// The sessions of a [`Server`].
const SESSIONS: u16 = 4;

/// A test server of open62541 on its own connection manager over `env::net`, with a
/// Double variable, for the clients of [`Server::connect`].
pub struct Server {
    clock: Clock,
    net: Net,
    url: String,
    /// Gives the next value of each client. The loop of a client sets the random
    /// numbers of open62541 on its thread, which the server also draws from.
    rng: RefCell<Rng>,
}

impl Server {
    /// Starts the server on `clock`, `net`, and `rng`, with its listener on `address`
    /// and port 4840 and room for 4 sessions, runs `body` while the server runs, then
    /// closes the server, also after a panic in `body`, which it then resumes. Each
    /// client of [`Server::connect`] also draws from `rng`.
    ///
    /// # Panics
    ///
    /// If the port is taken, or if open62541 refuses the server, its Double, a run of
    /// its loop, or a step of its close. A panic other than one of `body` leaks the
    /// manager and the server that it made by then, and a panic of the close replaces
    /// a panic of `body`.
    pub async fn scope<T>(
        clock: Clock,
        net: Net,
        address: IpAddr,
        rng: Rng,
        body: impl AsyncFnOnce(&Self) -> T,
    ) -> T {
        Self::serve(clock, net, address, rng, DOUBLE, body).await
    }

    /// As [`Server::scope`], with `double` as the value of [`VALUE`].
    async fn serve<T>(
        clock: Clock,
        net: Net,
        address: IpAddr,
        mut rng: Rng,
        double: f64,
        body: impl AsyncFnOnce(&Self) -> T,
    ) -> T {
        let local = SocketAddr::new(address, PORT);
        let manager = ManuallyDrop::new(
            connection::Manager::listening(
                Clock::clone(&clock),
                Net::clone(&net),
                local,
                u32::from(SESSIONS),
                &mut rng,
            )
            .expect("the port is free"),
        );
        // SAFETY: the close below deletes the server before the manager drops, and a
        // panic other than one of `body` leaks the manager.
        let server = unsafe { crate::server::Server::start(&manager, c"", SESSIONS) };
        server.add_double(VALUE, double);
        let this = Self {
            clock,
            net,
            url: format!("opc.tcp://{local}"),
            rng: RefCell::new(rng),
        };
        let value = {
            let mut drive = pin!(manager.drive(|_| {
                manager.events().run();
                Poll::<()>::Pending
            }));
            let mut body = pin!(super::caught(async { body(&this).await }));
            poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending(), "it never ends");
                body.as_mut().poll(cx)
            })
            .await
        };
        server.close(&manager).await;
        drop(ManuallyDrop::into_inner(manager));
        value.unwrap_or_else(|panic| panic::resume_unwind(panic))
    }

    /// Connects a client to the server, on its own connection manager.
    ///
    /// # Panics
    ///
    /// If the connect fails, with its status.
    pub async fn connect(&self) -> Client {
        let rng = &mut Rng::from_seed(self.rng.borrow_mut().next_u64());
        let (clock, net) = (Clock::clone(&self.clock), Net::clone(&self.net));
        match crate::client::Client::connect(clock, net, rng, &self.url).await {
            Ok(client) => Client(client),
            Err(status) => panic!("a connect failed: {status:?}"),
        }
    }
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// A client of a [`Server`].
pub struct Client(crate::client::Client);

impl Client {
    /// Reads the Double of the server.
    ///
    /// # Panics
    ///
    /// If the read fails, with its status, or gives another value.
    pub async fn read(&mut self) {
        match self.0.read(VALUE).await {
            Ok(value) => assert!(value.to_bits() == DOUBLE.to_bits(), "read {value}"),
            Err(status) => panic!("a read failed: {status:?}"),
        }
    }

    /// Closes the session and the connection of the client. A client that drops with no
    /// close holds one of the 4 sessions of its server until the session times out.
    pub async fn close(self) {
        self.0.close().await;
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use env::rng::Rng;
    use sim::node::Node;
    use sim::{Sim, node};
    use types::time::Span;

    use super::super::caught;
    use super::{PORT, Server};
    use crate::peer::session;

    /// Runs `body` on a node of a sim, and gives the error of the run as text.
    fn run<T: Send + 'static>(
        body: impl AsyncFnOnce(Node) -> T + Send + 'static,
    ) -> Result<T, String> {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        sim.run_on(&node, move |node, _| body(node))
            .map_err(|error| error.to_string())
    }

    /// Runs `body` in the scope of a server on `node`, with a new `Rng` from the
    /// entropy of `node`.
    async fn scope<T>(node: &Node, body: impl AsyncFnOnce(&Server) -> T) -> T {
        let (clock, net, address) = (node.clock(), node.net(), node.addresses()[0]);
        Server::scope(clock, net, address, node.entropy().rng(), body).await
    }

    /// The error of a run of seed 0 whose task panics with `message`.
    fn panicked(message: &str) -> Result<(), String> {
        Err(format!(
            "thread run_on panicked: {message}; replay with seed 0x0"
        ))
    }

    /// `count` minutes.
    fn minutes(count: i64) -> Span {
        Span::from_nanos(count * 60 * 1_000_000_000)
    }

    /// Connects a client, reads, and closes it.
    async fn read(server: &Server) {
        let mut client = server.connect().await;
        client.read().await;
        client.close().await;
    }

    /// Runs a scope whose `body` panics with "the body panics", which the scope
    /// resumes, then a scope at its address that connects, reads, and closes.
    fn frees_its_port(body: impl AsyncFnOnce(&Server) + Send + 'static) {
        let result = run(async |node| {
            let panic = caught(scope(&node, body)).await.expect_err("panics");
            assert_eq!(panic.downcast_ref(), Some(&"the body panics"));
            scope(&node, read).await;
        });
        assert_eq!(result, Ok(()));
    }

    /// A scope closes after a panic in its body, also with a client that drops with the
    /// body before the close, so the next scope takes its port.
    #[test]
    fn a_scope_whose_body_panics_frees_its_port() {
        frees_its_port(async |server: &Server| {
            let _client = server.connect().await;
            panic!("the body panics");
        });
    }

    /// A body that panics when it is called, before it gives its future.
    #[test]
    fn a_scope_whose_body_panics_at_its_call_frees_its_port() {
        frees_its_port(|_: &Server| -> std::future::Ready<()> {
            panic!("the body panics");
        });
    }

    /// The server has room for 4 sessions.
    #[test]
    fn a_fifth_client_of_a_scope_panics_with_its_status() {
        let result = run(async |node| {
            scope(&node, async |server: &Server| {
                let mut clients = Vec::new();
                for _ in 0..5 {
                    clients.push(server.connect().await);
                }
                for client in clients {
                    client.close().await;
                }
            })
            .await;
        });
        assert_eq!(result, panicked("a connect failed: BadTimeout"));
    }

    /// A client that drops with no close holds its session until the session times out,
    /// after 20 minutes.
    #[test]
    fn a_dropped_client_holds_its_session_until_it_times_out() {
        let fifth = |wait: Span| {
            run(async move |node| {
                let clock = node.clock();
                scope(&node, async |server: &Server| {
                    for _ in 0..4 {
                        drop(server.connect().await);
                    }
                    clock.sleep(wait).await;
                    server.connect().await.close().await;
                })
                .await;
            })
        };
        let held = panicked("a connect failed: BadTooManySessions");
        assert_eq!(fifth(minutes(19)), held);
        assert_eq!(fifth(minutes(21)), Ok(()));
    }

    /// The server closes a channel idle for 10 minutes.
    #[test]
    fn a_read_of_a_client_that_lost_its_channel_panics_with_its_status() {
        let result = run(async |node| {
            let clock = node.clock();
            scope(&node, async |server: &Server| {
                let mut client = server.connect().await;
                client.read().await;
                clock.sleep(minutes(11)).await;
                client.read().await;
                client.close().await;
            })
            .await;
        });
        assert_eq!(result, panicked("a read failed: BadSecureChannelClosed"));
    }

    #[test]
    fn a_second_scope_at_the_address_of_a_scope_panics() {
        let result = run(async |node| {
            scope(&node, async |_: &Server| {
                scope(&node, async |_: &Server| ()).await;
            })
            .await;
        });
        let free = "the port is free: AddressInUse { local: 10.0.0.1:4840 }";
        assert_eq!(result, panicked(free));
    }

    /// No public call makes a server serve another value, so this test serves 2.5
    /// through the private `Server::serve`. While a server serves `DOUBLE`, no caller
    /// of `Client::read` can see the removal of its check of the value, so only this
    /// test fails then.
    #[test]
    fn a_read_of_another_value_panics_with_it() {
        let result = run(async |node| {
            let (clock, net, address) = (node.clock(), node.net(), node.addresses()[0]);
            Server::serve(clock, net, address, node.entropy().rng(), 2.5, read).await;
        });
        assert_eq!(result, panicked("read 2.5"));
    }

    #[test]
    fn a_client_reads_the_double_of_its_server() {
        let debug = run(async |node| {
            scope(&node, async |server: &Server| {
                let mut client = server.connect().await;
                client.read().await;
                let debug = format!("{server:?} {client:?}");
                client.close().await;
                debug
            })
            .await
        });
        let url = "opc.tcp://10.0.0.1:4840";
        let debug = debug.expect("the run ends");
        assert_eq!(
            debug,
            format!(r#"Server {{ url: "{url}", .. }} Client {{ .. }}"#)
        );
    }

    /// Gives the ids of a session of a peer in a scope of each start value: before
    /// the first connect, and after each of two connects. The server draws them from
    /// the random numbers that the last loop on this thread set.
    fn sessions(starts: [u64; 2]) -> Vec<[[[u8; 16]; 2]; 3]> {
        run(async move |node| {
            let address = node.addresses()[0];
            let (net, remote) = (node.net(), SocketAddr::new(address, PORT));
            let mut ids = Vec::new();
            for start in starts {
                let body = async |server: &Server| {
                    let mut ids = [session(&net, remote).await; 3];
                    for id in &mut ids[1..] {
                        let client = server.connect().await;
                        *id = session(&net, remote).await;
                        client.close().await;
                    }
                    ids
                };
                let rng = Rng::from_seed(start);
                ids.push(
                    Server::scope(node.clock(), node.net(), address, rng, body).await,
                );
            }
            ids
        })
        .expect("the run ends")
    }

    #[test]
    fn each_scope_and_each_connect_take_another_value() {
        let ids = sessions([1, 2]);
        assert_ne!(ids[0][0], ids[1][0], "two scopes took the same value");
        assert_ne!(ids[0][1], ids[0][2], "two connects took the same value");
        let message = "the clients of two scopes took the same value";
        assert_ne!(ids[0][1], ids[1][1], "{message}");
    }

    #[test]
    fn scopes_of_one_start_value_give_a_peer_the_same_sessions() {
        let ids = sessions([1, 1]);
        assert_eq!(ids[0], ids[1], "a start value gave other sessions");
    }
}
