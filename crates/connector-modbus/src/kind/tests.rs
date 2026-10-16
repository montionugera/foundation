use connector::kind::Kind as _;
use types::time::Rate;

use super::*;
use crate::config::Read;
use crate::pdu::Table;
use crate::poll::Order;

fn read(channel: &str, address: u16) -> Read {
    Read {
        channel: channel.parse().expect("a name"),
        table: Table::HoldingRegisters,
        address,
        span: None,
    }
}

#[test]
fn check_gives_each_channel_as_a_write_in_order_and_the_counts() {
    let config = Config {
        host: "plc".into(),
        port: 502,
        unit: 1,
        rate: Rate::new(10, 1).expect("a rate"),
        order: Order::Big,
        reads: vec![read("plc.temp", 40), read("plc.flow", 2)],
    };
    let names = |names: &[&str]| -> Vec<_> {
        names
            .iter()
            .map(|name| name.parse().expect("a name"))
            .collect()
    };
    assert_eq!(
        Kind.check(&config),
        Ok(Channels {
            reader: None,
            writes: names(&["plc.temp", "plc.flow"]),
            counts: names(&["in", "missed", "lost"]),
        })
    );
}

#[test]
fn parse_reads_the_config() {
    let refused = Kind.parse(&Document::default()).map(|_| ());
    let codes =
        refused.map_err(|all| all.iter().map(|one| one.code).collect::<Vec<_>>());
    let code = document::diagnostic::Code::new;
    let missing = code("document.missing-attribute");
    assert_eq!(codes, Err(vec![missing, missing, code("modbus.no-read")]));
}

#[test]
fn discover_finds_nothing() {
    let token = cancel::Token::new();
    let found = Kind.discover(&token);
    let found = std::pin::pin!(found);
    let waker = std::task::Waker::noop();
    let polled = found.poll(&mut std::task::Context::from_waker(waker));
    assert!(
        matches!(polled, std::task::Poll::Ready(Ok(ref found)) if found.is_empty()),
        "{polled:?}"
    );
}
