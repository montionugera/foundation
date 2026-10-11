//! Breaker: a node stop at the instant a control lease ends.

use ::hub::writer;
use types::authority::Authority;

use super::hub::{I64, data, define, index, write};
use super::*;

const LEASE: Span = Span::from_nanos(1_000_000);

fn leased(subject: &str) -> writer::Config {
    writer::Config {
        subject: subject.parse().expect("a valid name"),
        authority: Authority(1),
        lease: Some(LEASE),
        channels: vec!["value".parse().expect("a valid name")],
    }
}

/// A stop at the instant a lease ends, with a waiter, ends the node, and the node
/// starts again on the same host.
#[test]
fn breaker_a_stop_as_a_lease_ends_ends_the_node() {
    for seed in 0..32 {
        let mut sim = sim::Sim::new(sim::Config {
            seed,
            ..sim::Config::default()
        });
        let host = host(&mut sim, 2);
        let node = Node::start(config(&host, Size::MEBIBYTE, Box::new(heap)));
        let stopper = node.stopper();
        let clock = host.clock();
        node.spawn(move |hub| async move {
            define(&hub, &[("time", index(1)), ("value", data(2, I64, 1))]);
            let mut a = hub.writer(leased("subject-a")).await.expect("opens");
            let _b = hub.writer(leased("subject-b")).await.expect("opens");
            let stamp = a.now().nanos();
            write(&mut a, stamp, 1);
            clock.sleep(LEASE).await;
            stopper.stop();
            std::future::pending::<()>().await;
        });
        assert_eq!(sim.run(), Ok(()), "seed {seed}");
        assert_eq!(node.join(), Ok(()), "seed {seed}");
        assert_eq!(run_on(&mut sim, &host), Ok(()), "seed {seed}");
    }
}
