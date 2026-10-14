//! Runs a test server of open62541 with clients that each have their own connection
//! manager, as the kind of OPC UA uses them.

use std::cell::RefCell;
use std::ffi::CString;
use std::net::{IpAddr, SocketAddr};

use env::clock::Clock;
use env::net::Net;
use env::rng::Rng;

use super::PORT;
use crate::client::Nodes;
use crate::connection;
use crate::event;
use crate::node::{Key, Name, Node};
use crate::value::Value;

/// The Double variable of a [`Server`].
const VALUE: Node = Node {
    namespace: 1,
    key: Key::Number(1000),
};
/// The name of the Double variable of a [`Server`] that has a name key.
const NAME: &str = "SpikeData";
/// The value of each variable.
const DOUBLE: f64 = 1.5;
/// The sessions of a [`Server`].
const SESSIONS: u16 = 4;

/// A variable of a [`Server`].
#[derive(Clone, Copy, Debug)]
pub enum Variable {
    /// `ns=1;i=1000`.
    Number,
    /// `ns=1;s=SpikeData`.
    Name,
}

/// A test server of open62541 on its own connection manager over `env::net`, with a
/// Double variable of each [`Variable`], for the clients of [`Server::connect`].
pub struct Server {
    clock: Clock,
    net: Net,
    url: CString,
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
    ///
    /// # Aborts
    ///
    /// When open62541 cannot allocate the loop or the connection manager: the process
    /// stops as the Rust allocator does when it has no memory.
    pub async fn scope<T>(
        clock: Clock,
        net: Net,
        address: IpAddr,
        mut rng: Rng,
        body: impl AsyncFnOnce(&Self) -> T,
    ) -> T {
        let local = SocketAddr::new(address, PORT);
        let manager = connection::Manager::listening(
            event::Time::monotonic(Clock::clone(&clock)),
            Net::clone(&net),
            local,
            u32::from(SESSIONS),
            &mut rng,
        )
        .expect("the port is free");
        let this = Self {
            clock,
            net,
            url: CString::new(format!("opc.tcp://{local}")).expect("no NUL"),
            rng: RefCell::new(rng),
        };
        let add = |server: &mut crate::server::Server| {
            server.add_scalar(&VALUE, Value::F64(DOUBLE));
            server.add_scalar(&named(), Value::F64(DOUBLE));
        };
        crate::server::Server::serve(manager, SESSIONS, add, body(&this)).await
    }

    /// Connects a client to the server, on its own connection manager, whose reads
    /// each read `count` copies of a [`Variable`] in one request.
    ///
    /// # Panics
    ///
    /// If the connect fails, with its status, or if the request or its answer of
    /// `count` copies needs more than one message of the session.
    ///
    /// # Aborts
    ///
    /// When open62541 cannot allocate the loop, the connection manager, the client, or
    /// a Read request: the process stops as the Rust allocator does when it has no
    /// memory.
    pub async fn connect(&self, count: usize) -> Client {
        let rng = &mut Rng::from_seed(self.rng.borrow_mut().next_u64());
        let time = event::Time::monotonic(Clock::clone(&self.clock));
        let net = Net::clone(&self.net);
        let connect = crate::client::Client::connect(time, net, rng, &self.url);
        let session = match connect.await {
            Ok(session) => session,
            Err(status) => panic!("a connect failed: {status:?}"),
        };
        let nodes = |node: Node| {
            let nodes: Vec<Node> = std::iter::repeat_n(node, count).collect();
            session.nodes(&nodes).expect("the nodes fit in one message")
        };
        let (number, name) = (nodes(VALUE), nodes(named()));
        Client {
            session,
            number,
            name,
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

/// The node of [`Variable::Name`].
fn named() -> Node {
    Node {
        namespace: 1,
        key: Key::Name(Name::new(NAME).expect("SpikeData is a name")),
    }
}

/// A client of a [`Server`].
pub struct Client {
    session: crate::client::Client,
    number: Nodes,
    name: Nodes,
}

impl Client {
    /// Reads the Double of each copy of `variable` in one request.
    ///
    /// # Panics
    ///
    /// If the read fails, with its status, or gives another value.
    pub async fn read(&mut self, variable: Variable) {
        let nodes = match variable {
            Variable::Number => &self.number,
            Variable::Name => &self.name,
        };
        let values = match self.session.read(nodes).await {
            Ok(values) => values,
            Err(status) => panic!("a read failed: {status:?}"),
        };
        for value in values {
            match value {
                Ok(Value::F64(value)) => {
                    assert!(value.to_bits() == DOUBLE.to_bits(), "read {value}");
                }
                Ok(value) => panic!("read {value:?}"),
                Err(status) => panic!("a read failed: {status:?}"),
            }
        }
    }

    /// Closes the session and the connection of the client. A client that drops with no
    /// close holds one of the 4 sessions of its server until the session times out.
    pub async fn close(self) {
        self.session.close().await;
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

    use super::{PORT, Server, Variable};
    use crate::peer::session;

    #[test]
    fn a_client_reads_each_double_of_its_server() {
        let mut sim = Sim::new(sim::Config::default());
        let node = sim.node(node::Config::default());
        sim.run_on(&node, move |node, _| async move {
            let address = node.addresses()[0];
            let body = async |server: &Server| {
                let mut client = server.connect(3).await;
                client.read(Variable::Number).await;
                client.read(Variable::Name).await;
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
                        let client = server.connect(1).await;
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
