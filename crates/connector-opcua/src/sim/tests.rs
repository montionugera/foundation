use std::net::SocketAddr;

use ::sim::{Sim, node};
use types::quality::Quality;

use super::{Served, Server, Variables};
use crate::client::Client;
use crate::ffi::Status;
use crate::node::Node;

const SPIKE: &str = "ns=3;s=SpikeData";
const DIP: &str = "ns=3;i=7";
const BAD: Quality = Quality(0x8000_0000);

#[test]
fn a_server_answers_each_read_with_its_variable_and_records_it() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let local = SocketAddr::new(node.addresses()[0], 4840);
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE, DIP]);
        let server =
            Server::listen(node.clock(), node.net(), local, &mut rng, &variables)
                .expect("the port is free");
        assert_eq!(server.local(), local);
        assert_eq!(
            format!("{server:?}"),
            format!("Server {{ local: {local}, .. }}")
        );
        let (clock, spike) = (node.clock(), SPIKE.parse::<Node>().expect("a node"));
        let mut times = Vec::new();
        let mut reads = Vec::new();
        server
            .serve(async {
                let url = format!("opc.tcp://{local}");
                let mut client =
                    (Client::connect(node.clock(), node.net(), &mut rng, &url))
                        .await
                        .expect("connect");
                for quality in [Quality::GOOD, BAD] {
                    variables.change(quality);
                    let before = clock.now();
                    reads.push(client.read(&spike).await);
                    times.push((before, clock.now()));
                }
                client.close().await;
            })
            .await;
        assert_eq!(reads, [Ok(1.0), Err(Status(BAD.0))]);
        let record = variables.served();
        assert_eq!(record[DIP], []);
        let spike = &record[SPIKE];
        let answers: Vec<_> = (spike.iter())
            .map(|served| (served.value, served.quality))
            .collect();
        assert_eq!(answers, [(1.0, Quality::GOOD), (2.0, BAD)]);
        for (Served { time, .. }, (before, after)) in spike.iter().zip(times) {
            assert!(
                before <= *time && *time <= after,
                "{time:?} in {before:?}..{after:?}"
            );
        }
        let again =
            Server::listen(node.clock(), node.net(), local, &mut rng, &variables);
        again
            .expect("a served server frees its port")
            .serve(async {})
            .await;
    })
    .expect("the run ends");
}

#[test]
#[should_panic(expected = r#""ns=3;x=1" is not a node id: "#)]
fn variables_panic_on_a_text_that_is_not_a_node_id() {
    drop(Variables::new(&[SPIKE, "ns=3;x=1"]));
}

#[test]
#[should_panic(expected = r#""ns=03;i=7" and "ns=3;i=7" name the same node"#)]
fn variables_panic_on_two_texts_of_one_node() {
    drop(Variables::new(&[DIP, "ns=03;i=7"]));
}
