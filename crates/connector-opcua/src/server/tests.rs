use std::net::SocketAddr;

use ::sim::{Sim, node};

use super::{READ, Server, Variable};
use crate::connection::Manager;
use crate::event;
use crate::node::{Node, ns0};
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
        let panic = caught(Server::serve(manager, 1, add, async {}))
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

/// A value whose mask names no built-in type (63), or that ends before its Double,
/// is not a `Variant`.
#[test]
fn an_add_of_a_value_that_is_not_a_variant_panics() {
    for value in [vec![63], vec![11, 0, 0]] {
        let mut sim = Sim::new(::sim::Config::default());
        let node = sim.node(node::Config::default());
        let message = sim
            .run_on(&node, move |node, _| async move {
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
                let variable = Variable {
                    node: "ns=1;i=7".parse().expect("a node"),
                    parent: Node::ns0(ns0::OBJECTS),
                    reference: ns0::ORGANIZES,
                    name: c"v",
                    data_type: Node::ns0(ns0::BASE_DATA_TYPE),
                    rank: -1,
                    access: READ,
                    value: Some(value),
                };
                let add = |server: &mut Server| server.add_variable(&variable);
                let panic = caught(Server::serve(manager, 1, add, async {}))
                    .await
                    .expect_err("the serve panics");
                panic.downcast_ref::<String>().cloned()
            })
            .expect("the run ends")
            .expect("a message");
        assert!(
            message.starts_with(
                "assertion `left == right` failed: open62541 refused the node"
            ),
            "{message}"
        );
    }
}

#[cfg(feature = "sim")]
#[test]
fn a_source_is_a_variable_of_the_data_type_base_data_type() {
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
            assert_eq!(server.data_type(&source), ns0::BASE_DATA_TYPE);
        };
        Server::serve(manager, 1, add, async {}).await;
    })
    .expect("the run ends");
}
