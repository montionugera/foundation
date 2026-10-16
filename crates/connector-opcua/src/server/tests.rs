use std::net::SocketAddr;

use ::sim::{Sim, node};

use super::Server;
use crate::connection::Manager;
use crate::event;
use crate::node::Node;
use crate::unwind::caught;
use crate::value::Value;

#[test]
fn an_add_of_a_node_of_the_number_0_panics_and_frees_the_port() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let local = SocketAddr::new(node.addresses()[0], 4840);
        let manager = Manager::listening(
            event::Time::monotonic(node.clock()),
            node.net(),
            local,
            4,
            &mut rng,
        )
        .expect("the port is free");
        let zero: Node = "ns=1;i=0".parse().expect("a node");
        let add = |server: &mut Server| server.add_scalar(&zero, Value::F64(1.0));
        let panic = caught(Server::serve(manager, 1, add, |_| false, async {}))
            .await
            .expect_err("the serve panics");
        assert_eq!(
            panic.downcast_ref::<String>().map(String::as_str),
            Some("ns=1;i=0: open62541 adds a node of the number 0 at a free number")
        );
        Manager::listening(
            event::Time::monotonic(node.clock()),
            node.net(),
            local,
            4,
            &mut rng,
        )
        .expect("the serve frees the port");
    })
    .expect("the run ends");
}

#[cfg(feature = "sim")]
#[test]
fn a_source_is_a_variable_of_the_data_type_base_data_type() {
    /// `UA_NS0ID_BASEDATATYPE`.
    const BASE_DATA_TYPE: u32 = 24;
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let local = SocketAddr::new(node.addresses()[0], 4840);
        let manager = Manager::listening(
            event::Time::monotonic(node.clock()),
            node.net(),
            local,
            4,
            &mut rng,
        )
        .expect("the port is free");
        let source: Node = "ns=1;i=7".parse().expect("a node");
        let add = |server: &mut Server| {
            server
                .add_source(&source, || (Value::I32(1), types::quality::Quality::GOOD));
            assert_eq!(server.data_type(&source), BASE_DATA_TYPE);
        };
        Server::serve(manager, 1, add, |_| false, async {}).await;
    })
    .expect("the run ends");
}
