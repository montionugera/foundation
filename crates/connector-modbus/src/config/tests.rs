#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use document::value::Kind as Value;
use document::{Attribute, Label, Map, Position, Source};
use proptest::prelude::*;

use super::*;

/// A one-byte span at `offset`.
fn at(offset: u32) -> Option<Span> {
    let position = |offset| Position {
        offset,
        line: 0,
        column: offset,
    };
    Span::new(Source(0), position(offset), position(offset + 1))
}

fn string(text: &str) -> Value {
    Value::String(text.into())
}

/// A document with `attributes`, each key at its offset and its value one byte after.
fn document(attributes: &[(u32, &str, Value)], blocks: Vec<Block>) -> Document {
    let attributes = attributes
        .iter()
        .map(|(offset, key, kind)| Attribute {
            key: (*key).into(),
            key_span: at(*offset),
            value: document::value::Value {
                kind: kind.clone(),
                span: at(offset + 1),
            },
        })
        .collect();
    Document {
        attributes: Map::new(attributes).expect("unique keys"),
        blocks,
    }
}

/// A block with `keyword` at `offset`, each label one byte after the one before, and
/// `attributes`.
fn block(
    offset: u32,
    keyword: &str,
    labels: &[&str],
    attributes: &[(u32, &str, Value)],
) -> Block {
    let labels = (offset + 1..).zip(labels).map(|(offset, text)| Label {
        text: (*text).into(),
        span: at(offset),
    });
    Block {
        keyword: keyword.into(),
        keyword_span: at(offset),
        labels: labels.collect(),
        body: document(attributes, Vec::new()),
        span: at(offset),
    }
}

/// A `read` block of `channel` at `offset`, with `table` at `offset + 10` and
/// `address` at `offset + 12`.
fn read(offset: u32, channel: &str, table: Value, address: Value) -> Block {
    block(
        offset,
        "read",
        &[channel],
        &[
            (offset + 10, "table", table),
            (offset + 12, "address", address),
        ],
    )
}

/// A `read` block of `plc.temp` at offset 20, of holding register 40.
fn temp() -> Block {
    read(20, "plc.temp", string("holding"), Value::Integer(40))
}

/// The config of the connector at `address`, with `others` and `reads`.
fn config(
    address: Value,
    others: &[(u32, &str, Value)],
    reads: Vec<Block>,
) -> Document {
    let mut attributes = vec![(0, "address", address), (10, "rate", string("10Hz"))];
    attributes.extend_from_slice(others);
    document(&attributes, reads)
}

fn refused(code: &'static str, at: u32, message: &str, fix: &str) -> Diagnostic {
    Diagnostic::new(Code::new(code), self::at(at), message.into(), fix.into())
}

fn name(text: &str) -> Name {
    text.parse().expect("a name")
}

#[test]
fn reads_each_attribute_and_each_read() {
    let reads = vec![
        temp(),
        read(40, "plc.run", string("coil"), Value::Integer(0)),
        read(60, "plc.door", string("discrete"), Value::Integer(65535)),
        read(80, "plc.flow", string("input"), Value::Integer(7)),
    ];
    let others = [
        (14, "unit", Value::Integer(17)),
        (16, "order", string("little")),
    ];
    let read = |channel, table, address, offset| Read {
        channel: name(channel),
        table,
        address,
        span: at(offset),
    };
    assert_eq!(
        parse(&config(string("10.0.0.7:1502"), &others, reads)),
        Ok(Config {
            host: "10.0.0.7".into(),
            port: 1502,
            unit: 17,
            rate: Rate::new(10, 1).expect("a rate"),
            order: Order::Little,
            reads: vec![
                read("plc.temp", Table::HoldingRegisters, 40, 20),
                read("plc.run", Table::Coils, 0, 40),
                read("plc.door", Table::DiscreteInputs, 65535, 60),
                read("plc.flow", Table::InputRegisters, 7, 80),
            ],
        })
    );
}

#[test]
fn gives_port_502_unit_1_and_the_big_order_by_default() {
    let config = parse(&config(string("plc"), &[], vec![temp()])).expect("a config");
    assert_eq!(
        (&*config.host, config.port, config.unit, config.order),
        ("plc", 502, 1, Order::Big)
    );
}

#[test]
fn reads_an_ipv6_host_in_brackets() {
    for (address, port) in [("[fd00::1]:1502", 1502), ("[fd00::1]", 502)] {
        let config = parse(&config(string(address), &[], vec![temp()]));
        let config = config.expect("a config");
        assert_eq!((&*config.host, config.port), ("fd00::1", port), "{address}");
    }
}

#[test]
fn refuses_a_bad_address() {
    let fix = "Write a host and a port, such as \"plc:502\" or \"[fd00::1]:502\"";
    let port = |address: &str| {
        format!("the address {address:?} has a port that is not 0 to 65535")
    };
    for (address, message) in [
        (
            Value::Integer(502),
            "an address is a string, not an integer".into(),
        ),
        (
            Value::Reference(name("plc")),
            "an address is a string, not a reference".into(),
        ),
        (string(""), "the address \"\" has no host".into()),
        (string(":502"), "the address \":502\" has no host".into()),
        (
            string("[]:502"),
            "the address \"[]:502\" has no host".into(),
        ),
        (string("plc:"), port("plc:")),
        (string("plc:65536"), port("plc:65536")),
        (string("plc:+502"), port("plc:+502")),
        (string("plc:x"), port("plc:x")),
        (string("[fd00::1]:"), port("[fd00::1]:")),
        (
            string("fd00::1"),
            "the address \"fd00::1\" has more than one `:`. Put an IPv6 host in \
             brackets"
                .into(),
        ),
        (
            string("[fd00::1"),
            "the address \"[fd00::1\" has no `]`".into(),
        ),
        (
            string("[fd00::1]502"),
            "the address \"[fd00::1]502\" has \"502\" after its `]`, not a `:` and a \
             port"
                .into(),
        ),
    ] {
        assert_eq!(
            parse(&config(address.clone(), &[], vec![temp()])),
            Err(vec![refused("modbus.bad-address", 1, &message, fix)]),
            "{address:?}"
        );
    }
}

#[test]
fn refuses_a_bad_unit() {
    let fix = "Write the unit of the device, 0 to 255, such as 1";
    for (unit, message) in [
        (Value::Integer(-1), "the unit -1 is not 0 to 255"),
        (Value::Integer(256), "the unit 256 is not 0 to 255"),
        (string("1"), "a unit is an integer, not a string"),
    ] {
        let others = [(14, "unit", unit.clone())];
        assert_eq!(
            parse(&config(string("plc"), &others, vec![temp()])),
            Err(vec![refused("modbus.bad-unit", 15, message, fix)]),
            "{unit:?}"
        );
    }
}

#[test]
fn reads_units_0_and_255() {
    for unit in [0, 255] {
        let others = [(14, "unit", Value::Integer(unit))];
        let config = parse(&config(string("plc"), &others, vec![temp()]));
        assert_eq!(config.map(|config| config.unit.into()), Ok(unit));
    }
}

#[test]
fn refuses_a_bad_order() {
    let fix =
        "Write \"big\" when the first register holds the high word, or \"little\"";
    for (order, shown) in [
        (string("middle"), "\"middle\""),
        (string("Big"), "\"Big\""),
        (Value::Bool(true), "a bool"),
    ] {
        let others = [(16, "order", order.clone())];
        let message = format!("an order is \"big\" or \"little\", not {shown}");
        assert_eq!(
            parse(&config(string("plc"), &others, vec![temp()])),
            Err(vec![refused("modbus.bad-order", 17, &message, fix)]),
            "{order:?}"
        );
    }
}

#[test]
fn refuses_a_bad_table_or_register_address() {
    let tables = "\"coil\", \"discrete\", \"holding\", or \"input\"";
    let table_fix = "Write the table of the value, such as \"holding\"";
    let address_fix = "Write the address of the protocol, from 0, such as 40 for the \
                       41st holding register";
    for (table, address, refusal) in [
        (
            string("holdings"),
            Value::Integer(40),
            refused(
                "modbus.bad-table",
                31,
                &format!("a table is {tables}, not \"holdings\""),
                table_fix,
            ),
        ),
        (
            Value::Integer(3),
            Value::Integer(40),
            refused(
                "modbus.bad-table",
                31,
                &format!("a table is {tables}, not an integer"),
                table_fix,
            ),
        ),
        (
            string("holding"),
            Value::Integer(-1),
            refused(
                "modbus.bad-register",
                33,
                "the address -1 is not 0 to 65535",
                address_fix,
            ),
        ),
        (
            string("holding"),
            Value::Integer(65536),
            refused(
                "modbus.bad-register",
                33,
                "the address 65536 is not 0 to 65535",
                address_fix,
            ),
        ),
        (
            string("holding"),
            string("40001"),
            refused(
                "modbus.bad-register",
                33,
                "an address is an integer, not a string",
                address_fix,
            ),
        ),
    ] {
        let reads = vec![read(20, "plc.temp", table, address)];
        assert_eq!(
            parse(&config(string("plc"), &[], reads)),
            Err(vec![refusal])
        );
    }
}

#[test]
fn refuses_a_config_with_no_address_rate_or_read() {
    let no_read = Diagnostic::new(
        Code::new("modbus.no-read"),
        None,
        "the connector has no `read` block".into(),
        "Add a `read` block for each channel, such as `read \"plc.temp\" { table = \
         \"holding\", address = 40 }`"
            .into(),
    );
    let missing = |key: &str, fix: &str| {
        Diagnostic::new(
            Code::new("document.missing-attribute"),
            None,
            format!("the connector has no `{key}`"),
            fix.into(),
        )
    };
    assert_eq!(
        parse(&document(&[], Vec::new())),
        Err(vec![
            missing(
                "address",
                "Add an `address` attribute with the host and port of the device, \
                 such as \"plc:502\"",
            ),
            missing(
                "rate",
                "Add a `rate` attribute with the reads per second, such as \"10Hz\"",
            ),
            no_read,
        ])
    );
}

#[test]
fn refuses_a_read_with_no_table_or_address_or_an_unknown_key() {
    let reads = vec![block(
        20,
        "read",
        &["plc.temp"],
        &[(30, "register", Value::Integer(40))],
    )];
    let missing = |key: &str, fix: &str| {
        refused(
            "document.missing-attribute",
            20,
            &format!("the `read` block has no `{key}`"),
            fix,
        )
    };
    assert_eq!(
        parse(&config(
            string("plc"),
            &[(14, "port", Value::Integer(502))],
            reads
        )),
        Err(vec![
            missing(
                "table",
                "Add a `table` attribute with the table of the value, such as \
                 \"holding\"",
            ),
            missing(
                "address",
                "Add an `address` attribute with the address of the value, such as 40",
            ),
            refused(
                "document.unknown-attribute",
                30,
                "`register` is not an attribute of the `read` block",
                "Use `address` or `table`, or remove it",
            ),
            refused(
                "document.unknown-attribute",
                14,
                "`port` is not an attribute of the connector",
                "Use `address`, `order`, `rate`, or `unit`, or remove it",
            ),
        ])
    );
}

#[test]
fn refuses_a_channel_in_two_read_blocks() {
    let reads = vec![
        temp(),
        read(40, "plc.temp", string("input"), Value::Integer(1)),
    ];
    let mut repeated = refused(
        "modbus.repeated-channel",
        41,
        "the connector reads `plc.temp` in another `read` block",
        "Remove one of the two `read` blocks",
    );
    repeated.notes.push(Note {
        span: at(21).expect("a span"),
        text: "the first `read` block of `plc.temp`".into(),
    });
    assert_eq!(
        parse(&config(string("plc"), &[], reads)),
        Err(vec![repeated])
    );
}

proptest! {
    #[test]
    fn reads_each_host_and_port(
        host in "[a-z0-9.-]{1,20}",
        ipv6 in any::<std::net::Ipv6Addr>(),
        port in any::<u16>(),
    ) {
        let ipv6 = ipv6.to_string();
        for (address, want) in [
            (format!("{host}:{port}"), (&*host, port)),
            (host.clone(), (&*host, 502)),
            (format!("[{ipv6}]:{port}"), (&*ipv6, port)),
        ] {
            let config = parse(&config(string(&address), &[], vec![temp()]));
            let config = config.expect("a config");
            prop_assert_eq!((&*config.host, config.port), want);
        }
    }
}
