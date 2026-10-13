use std::future::poll_fn;
use std::net::SocketAddr;
use std::panic::{self, AssertUnwindSafe};
use std::pin::pin;
use std::task::Poll;

use ::sim::{Sim, node};

use super::Server;
use crate::connection::Manager;
use crate::node::Node;

#[test]
fn an_add_of_a_node_of_the_number_0_panics_and_frees_the_port() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let local = SocketAddr::new(node.addresses()[0], 4840);
        let manager = Manager::listening(node.clock(), node.net(), local, 4, &mut rng)
            .expect("the port is free");
        let zero: Node = "ns=1;i=0".parse().expect("a node");
        let add = |server: &mut Server| server.add_double(&zero, 1.0);
        let mut serve = pin!(Server::serve(manager, 1, add, async {}));
        let panic = poll_fn(|cx| {
            match panic::catch_unwind(AssertUnwindSafe(|| serve.as_mut().poll(cx))) {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(())) => panic!("the serve ends with no panic"),
                Err(panic) => Poll::Ready(panic),
            }
        })
        .await;
        assert_eq!(
            panic.downcast_ref::<String>().map(String::as_str),
            Some("ns=1;i=0: open62541 adds a node of the number 0 at a free number")
        );
        Manager::listening(node.clock(), node.net(), local, 4, &mut rng)
            .expect("the serve frees the port");
    })
    .expect("the run ends");
}
