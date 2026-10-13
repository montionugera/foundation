use std::net::SocketAddr;

use ::sim::{Sim, node};

use super::Server;
use crate::connection::Manager;
use crate::node::Node;

#[test]
fn an_add_of_a_node_of_the_number_0_panics() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let local = SocketAddr::new(node.addresses()[0], 4840);
        let manager = Manager::listening(node.clock(), node.net(), local, 4, &mut rng)
            .expect("the port is free");
        let zero: Node = "ns=3;i=0".parse().expect("a node");
        let add = |server: &mut Server| server.add_double(&zero, 1.0);
        Server::serve(manager, 1, add, async {}).await;
    });
    let Err(::sim::Error::Panicked { message, .. }) = run else {
        panic!("the add panics: {run:?}");
    };
    assert_eq!(
        message,
        "ns=3;i=0: open62541 adds a node of the number 0 at a free number"
    );
}
