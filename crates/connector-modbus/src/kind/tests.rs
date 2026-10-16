use connector::kind::Kind as _;
use document::value::{Kind as Of, Value};
use document::{Attribute, Block, Label, Map};

use super::*;

/// An attribute of `key` with `kind` and no span.
fn attribute(key: &str, kind: Of) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value: Value { kind, span: None },
    }
}

/// The config of a connector at `plc` with a `read` block of each channel.
fn config(channels: &[&str]) -> Document {
    let read = |(address, channel): (i128, &&str)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![Label {
            text: (*channel).into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![
                attribute("table", Of::String("holding".into())),
                attribute("address", Of::Integer(address)),
            ])
            .expect("unique keys"),
            blocks: Vec::new(),
        },
        span: None,
    };
    Document {
        attributes: Map::new(vec![
            attribute("address", Of::String("plc".into())),
            attribute("rate", Of::String("10Hz".into())),
        ])
        .expect("unique keys"),
        blocks: (0..).zip(channels).map(read).collect(),
    }
}

#[test]
fn check_gives_each_channel_as_a_write_in_order_and_the_counts() {
    let config = Kind
        .parse(&config(&["plc.temp", "plc.flow"]))
        .expect("a config");
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
fn the_table_of_kinds_checks_the_kind() {
    let table = kind::Table::new().with("modbus", Kind);
    let checked = table.check("modbus", None, &config(&["plc.temp"]));
    assert_eq!(checked.map(|channels| channels.writes.len()), Ok(1));
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
