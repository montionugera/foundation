//! Runs a test server of open62541 with clients that each have their own connection
//! manager, as the kind of OPC UA uses them.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::RefCell;
use std::future::{Future as _, poll_fn};
use std::mem::ManuallyDrop;
use std::net::{IpAddr, SocketAddr};
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
    /// closes the server. Each client of [`Server::connect`] also draws from `rng`.
    ///
    /// # Panics
    ///
    /// If the port is taken, or if open62541 refuses the server or a step of its
    /// close. A panic in `body` leaks the server and its manager.
    pub async fn scope<T>(
        clock: Clock,
        net: Net,
        address: IpAddr,
        mut rng: Rng,
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
        // panic leaks the manager.
        let server = unsafe { crate::server::Server::start(&manager, c"", SESSIONS) };
        server.add_double(VALUE, DOUBLE);
        let this = Self {
            clock,
            net,
            url: format!("opc.tcp://{local}"),
            rng: RefCell::new(rng),
        };
        let value = {
            let mut serve = pin!(manager.drive(|_| {
                manager.events().run();
                Poll::<()>::Pending
            }));
            let mut body = pin!(body(&this));
            poll_fn(|cx| {
                assert!(serve.as_mut().poll(cx).is_pending(), "it never ends");
                body.as_mut().poll(cx)
            })
            .await
        };
        server.close(&manager).await;
        drop(ManuallyDrop::into_inner(manager));
        value
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
    use env::rng::Rng;
    use sim::{Sim, node};

    use super::Server;

    #[test]
    fn each_scope_draws_from_its_rng() {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        let drawn = sim
            .run_on(&node, move |node, _| async move {
                let address = node.addresses()[0];
                let mut drawn = Vec::new();
                for start in [1, 2] {
                    let body = async |server: &Server| {
                        // SAFETY: it draws from the generator of this thread.
                        let before = unsafe { crate::ffi::test::UA_UInt32_random() };
                        let client = server.connect().await;
                        // SAFETY: as above.
                        let after = unsafe { crate::ffi::test::UA_UInt32_random() };
                        client.close().await;
                        (before, after)
                    };
                    let rng = Rng::from_seed(start);
                    drawn.push(
                        Server::scope(node.clock(), node.net(), address, rng, body)
                            .await,
                    );
                }
                drawn
            })
            .expect("the run ends");
        assert_ne!(drawn[0].0, drawn[1].0, "two scopes drew the same value");
        assert_ne!(
            drawn[0].1, drawn[1].1,
            "the clients of two scopes drew the same"
        );
    }

    #[test]
    fn a_client_reads_the_double_of_its_server() {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        sim.run_on(&node, move |node, _| async move {
            let address = node.addresses()[0];
            let body = async |server: &Server| {
                let mut client = server.connect().await;
                client.read().await;
                let debug = format!("{server:?} {client:?}");
                client.close().await;
                debug
            };
            let rng = node.entropy().rng();
            let debug =
                Server::scope(node.clock(), node.net(), address, rng, body).await;
            let url = format!("opc.tcp://{address}:4840");
            assert_eq!(
                debug,
                format!(r#"Server {{ url: "{url}", .. }} Client {{ .. }}"#)
            );
        })
        .expect("the run ends");
    }

    #[test]
    fn each_client_draws_other_values() {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        let drawn = sim
            .run_on(&node, move |node, _| async move {
                let address = node.addresses()[0];
                let body = async |server: &Server| {
                    let mut drawn = Vec::new();
                    for _ in 0..2 {
                        let client = server.connect().await;
                        // SAFETY: it draws from the generator of this thread.
                        drawn.push(unsafe { crate::ffi::test::UA_UInt32_random() });
                        client.close().await;
                    }
                    drawn
                };
                let rng = node.entropy().rng();
                Server::scope(node.clock(), node.net(), address, rng, body).await
            })
            .expect("the run ends");
        assert_ne!(drawn[0], drawn[1], "two connects drew the same value");
    }
}
