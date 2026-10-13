use std::ffi::CString;
use std::net::SocketAddr;
use std::num::NonZeroUsize;

use ::sim::{Sim, node};
use env::net::{Listener, tcp};
use types::quality::Quality;

use super::{Served, Variables, serve};
use crate::client::Client;
use crate::event;
use crate::ffi::Status;
use crate::node::Node;

const SPIKE: &str = "ns=3;s=SpikeData";
const DIP: &str = "ns=3;i=7";
const BAD: Quality = Quality(0x8000_0000);

/// Binds a listener of `node` on its port 4840.
fn listen(node: &node::Node) -> Listener {
    let local = SocketAddr::new(node.addresses()[0], 4840);
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
    node.net().listen(&listen).expect("the port is free")
}

#[test]
fn a_server_answers_each_read_with_its_variable_and_records_it() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE, DIP]);
        let (clock, spike) = (node.clock(), SPIKE.parse::<Node>().expect("a node"));
        let mut times = Vec::new();
        let mut reads = Vec::new();
        let until = async {
            let mut client = (Client::connect(
                event::Time::monotonic(node.clock()),
                node.net(),
                &mut rng,
                &url,
            ))
            .await
            .expect("connect");
            for quality in [Quality::GOOD, BAD] {
                variables.change(quality);
                let before = clock.now();
                reads.push(client.read_one(&spike).await);
                times.push((before, clock.now()));
            }
            client.close().await;
        };
        let mut served = node.entropy().rng();
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
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
        let again = listen(&node);
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            again,
            &mut served,
            &variables,
            async {},
        )
        .await;
    })
    .expect("the run ends");
}

#[test]
fn a_server_has_room_for_4_sessions() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let listener = listen(&node);
        let url =
            CString::new(format!("opc.tcp://{}", listener.local())).expect("no NUL");
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE]);
        let spike = SPIKE.parse::<Node>().expect("a node");
        let mut reads = Vec::new();
        let mut fifth = None;
        let until = async {
            let mut clients = Vec::new();
            for _ in 0..4 {
                let connect = Client::connect(
                    event::Time::monotonic(node.clock()),
                    node.net(),
                    &mut rng,
                    &url,
                );
                clients.push(connect.await.expect("connect"));
            }
            for client in &mut clients {
                reads.push(client.read_one(&spike).await);
            }
            let connect = Client::connect(
                event::Time::monotonic(node.clock()),
                node.net(),
                &mut rng,
                &url,
            );
            fifth = connect.await.err();
            for client in clients {
                client.close().await;
            }
        };
        let mut served = node.entropy().rng();
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listener,
            &mut served,
            &variables,
            until,
        )
        .await;
        assert_eq!(reads, [Ok(0.0); 4]);
        assert_eq!(fifth, Some(Status::BAD_TIMEOUT));
        assert_eq!(variables.served()[SPIKE].len(), 4);
    })
    .expect("the run ends");
}

#[test]
fn a_server_panics_on_a_variable_of_the_number_0() {
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE, "ns=3;i=0"]);
        serve(
            node.clock(),
            event::Time::monotonic(node.clock()).wall,
            node.net(),
            listen(&node),
            &mut rng,
            &variables,
            async {},
        )
        .await;
    });
    let Err(::sim::Error::Panicked { message, .. }) = run else {
        panic!("the serve panics: {run:?}");
    };
    assert_eq!(
        message,
        "ns=3;i=0: open62541 adds a node of the number 0 at a free number"
    );
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

/// Serves with a `wall` that panics in a child process, and does nothing outside one.
#[test]
fn serves_with_a_wall_that_panics() {
    if !crate::child::running() {
        return;
    }
    let mut sim = Sim::new(::sim::Config::default());
    let node = sim.node(node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let mut rng = node.entropy().rng();
        let variables = Variables::new(&[SPIKE]);
        let wall = || -> types::time::Stamp { panic!("the wall fails") };
        let (clock, net) = (node.clock(), node.net());
        serve(
            clock,
            wall,
            net,
            listen(&node),
            &mut rng,
            &variables,
            async {},
        )
        .await;
    });
    drop(run);
}

#[test]
#[cfg(unix)]
fn a_panic_in_the_wall_of_a_server_aborts() {
    let output =
        crate::child::aborted("sim::tests::serves_with_a_wall_that_panics", &[]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("panic in a function that cannot unwind"),
        "{stderr}"
    );
}
