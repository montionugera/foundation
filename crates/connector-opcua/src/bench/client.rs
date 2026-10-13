//! Runs a test server of open62541 with clients that each have their own connection
//! manager, as the kind of OPC UA uses them.

use std::cell::RefCell;
use std::net::{IpAddr, SocketAddr};

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;

use super::PORT;
use crate::connection;
use crate::node::{Key, Node};

/// The Double variable of a [`Server`].
const VALUE: Node = Node {
    namespace: 1,
    key: Key::Number(1000),
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
        let manager = connection::Manager::listening(
            Clock::clone(&clock),
            Net::clone(&net),
            local,
            u32::from(SESSIONS),
            &mut rng,
        )
        .expect("the port is free");
        let this = Self {
            clock,
            net,
            url: format!("opc.tcp://{local}"),
            rng: RefCell::new(rng),
        };
        let add =
            |server: &mut crate::server::Server| server.add_double(&VALUE, DOUBLE);
        crate::server::Server::serve(manager, SESSIONS, add, body(&this)).await
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
        match self.0.read(&VALUE).await {
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
    use sim::{Sim, node};

    use super::{PORT, Server};
    use crate::peer::session;

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

    /// Gives the ids of a session of a peer in a scope of each start value: before
    /// the first connect, and after each of two connects. The server draws them from
    /// the random numbers that the last loop on this thread set.
    fn sessions(starts: [u64; 2]) -> Vec<[[[u8; 16]; 2]; 3]> {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        sim.run_on(&node, move |node, _| async move {
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
