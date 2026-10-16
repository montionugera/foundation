//! Some tests read the `pub(crate)` fields of a `Config`: until `run` (PR B2 of #432),
//! no public call shows them. `gives_another_config_for_each_other_value` and
//! `gives_the_defaults_as_written` show the same values through `PartialEq` alone.
#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use document::value::Kind as Value;
use document::{Attribute, Label, Map, Position, Source, Span};
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
/// `attributes`. The block itself spans from `offset + 100`, so a test tells the two
/// spans apart.
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
        span: at(offset + 100),
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
    let read = |channel, table, address| Read {
        channel: name(channel),
        table,
        address,
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
                read("plc.temp", Table::HoldingRegisters, 40),
                read("plc.run", Table::Coils, 0),
                read("plc.door", Table::DiscreteInputs, 65535),
                read("plc.flow", Table::InputRegisters, 7),
            ],
        })
    );
}

/// The example of MODBUS KIND parses as it states.
#[test]
fn reads_the_example_of_the_record() {
    let reads = vec![read(20, "plc.temp", string("holding"), Value::Integer(40))];
    let rate = [(10, "rate", string("10Hz"))];
    let mut example = document(&rate, reads);
    example.attributes = Map::new(vec![
        Attribute {
            key: "address".into(),
            key_span: at(0),
            value: document::value::Value {
                kind: string("plc:502"),
                span: at(1),
            },
        },
        example.attributes.get("rate").expect("a rate").clone(),
    ])
    .expect("unique keys");
    assert_eq!(
        parse(&example),
        Ok(Config {
            host: "plc".into(),
            port: 502,
            unit: 1,
            rate: Rate::new(10, 1).expect("a rate"),
            order: Order::Big,
            reads: vec![Read {
                channel: name("plc.temp"),
                table: Table::HoldingRegisters,
                address: 40,
            }],
        })
    );
}

#[test]
fn reads_a_table_and_an_order_as_references() {
    let reads = vec![read(
        20,
        "plc.run",
        Value::Reference(name("coil")),
        Value::Integer(3),
    )];
    let others = [(16, "order", Value::Reference(name("little")))];
    let written = [(16, "order", string("little"))];
    let strings = vec![read(20, "plc.run", string("coil"), Value::Integer(3))];
    let parsed = parse(&config(string("plc"), &others, reads));
    assert!(parsed.is_ok(), "{parsed:?}");
    assert_eq!(parsed, parse(&config(string("plc"), &written, strings)));
}

#[test]
fn gives_the_defaults_as_written() {
    let written = [
        (14, "unit", Value::Integer(1)),
        (16, "order", string("big")),
    ];
    assert_eq!(
        parse(&config(string("plc"), &[], vec![temp()])),
        parse(&config(string("plc:502"), &written, vec![temp()]))
    );
}

#[test]
fn gives_another_config_for_each_other_value() {
    let base = || {
        let others = [
            (14, "unit", Value::Integer(1)),
            (16, "order", string("big")),
        ];
        config(string("plc:502"), &others, vec![temp()])
    };
    let with = |key: &str, kind: Value| {
        let mut changed = base();
        let attributes = changed.attributes.iter().map(|attribute| {
            let mut attribute = attribute.clone();
            if &*attribute.key == key {
                attribute.value.kind = kind.clone();
            }
            attribute
        });
        changed.attributes = Map::new(attributes.collect()).expect("unique keys");
        changed
    };
    let reading = |read: Block| {
        let mut changed = base();
        changed.blocks = vec![read];
        changed
    };
    let parsed = parse(&base()).expect("a config");
    for other in [
        with("address", string("plc2:502")),
        with("address", string("plc:503")),
        with("unit", Value::Integer(2)),
        with("order", string("little")),
        with("rate", string("20Hz")),
        reading(read(20, "plc.flow", string("holding"), Value::Integer(40))),
        reading(read(20, "plc.temp", string("input"), Value::Integer(40))),
        reading(read(20, "plc.temp", string("holding"), Value::Integer(41))),
    ] {
        let other = parse(&other).expect("a config");
        assert_ne!(parsed, other);
    }
}

#[test]
fn gives_another_config_for_the_case_of_a_host_and_for_a_zone() {
    for (address, other) in [
        ("Line-2.site.local:502", "line-2.site.local:502"),
        ("[fe80::1%eth0]", "[fe80::1]"),
    ] {
        let parse = |address| parse(&config(string(address), &[], vec![temp()]));
        let (parsed, other) = (parse(address), parse(other));
        assert_ne!(
            parsed.expect("a config"),
            other.expect("a config"),
            "{address}"
        );
    }
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
fn reads_a_host_of_letters_digits_dashes_dots_and_underscores() {
    for (address, host) in [
        ("plc_1:502", "plc_1"),
        ("Line-2.site.local:502", "Line-2.site.local"),
        ("10.0.0.2:502", "10.0.0.2"),
    ] {
        let config = parse(&config(string(address), &[], vec![temp()]));
        let config = config.expect("a config");
        assert_eq!((&*config.host, config.port), (host, 502), "{address}");
    }
}

#[test]
fn reads_an_ipv6_host_in_brackets() {
    for (address, host, port) in [
        ("[fd00::1]:1502", "fd00::1", 1502),
        ("[fd00::1]", "fd00::1", 502),
        ("[fe80::1%eth0.100]:502", "fe80::1%eth0.100", 502),
        ("[fe80::1%2]", "fe80::1%2", 502),
    ] {
        let config = parse(&config(string(address), &[], vec![temp()]));
        let config = config.expect("a config");
        assert_eq!((&*config.host, config.port), (host, port), "{address}");
    }
}

#[test]
fn refuses_a_bad_address() {
    let fix = "Write a host and a port, such as \"plc:502\" or \"[fd00::1]:502\"";
    let port = |address: &str| {
        format!("the address {address:?} has a port that is not 1 to 65535")
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
        (string("plc:0"), port("plc:0")),
        (string("[fd00::1]:0"), port("[fd00::1]:0")),
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
            string("[fd00::1]]"),
            "the address \"[fd00::1]]\" has \"]\" after its `]`, not a `:` and a port"
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
fn refuses_a_host_that_is_not_a_name_or_an_ip_address() {
    let fix = "Write a host and a port, such as \"plc:502\" or \"[fd00::1]:502\"";
    let host = |address: &str| {
        format!(
            "the address {address:?} has a host with a character other than a letter, \
             a digit, `-`, `.`, or `_`"
        )
    };
    for (address, message) in [
        (
            string("[plc]:502"),
            "the address \"[plc]:502\" has \"plc\" in brackets, which is not an IPv6 \
             address"
                .into(),
        ),
        (
            string("[10.0.0.2]:502"),
            "the address \"[10.0.0.2]:502\" has \"10.0.0.2\" in brackets, which is \
             not an IPv6 address"
                .into(),
        ),
        (
            string("[fe80::1%]:502"),
            "the address \"[fe80::1%]:502\" has the zone \"\", which is not an \
             interface name or number"
                .into(),
        ),
        (
            string("[fe80::1%eth 0]"),
            "the address \"[fe80::1%eth 0]\" has the zone \"eth 0\", which is not an \
             interface name or number"
                .into(),
        ),
        (
            string("[plc%eth0]"),
            "the address \"[plc%eth0]\" has \"plc%eth0\" in brackets, which is not \
             an IPv6 address"
                .into(),
        ),
        (
            string("[[fd00::1]:502"),
            "the address \"[[fd00::1]:502\" has \"[fd00::1\" in brackets, which is \
             not an IPv6 address"
                .into(),
        ),
        (string("plc :502"), host("plc :502")),
        (string("pl c"), host("pl c")),
        (string("plc/x:502"), host("plc/x:502")),
        (string("plc]:502"), host("plc]:502")),
        (string("p[lc:502"), host("p[lc:502")),
    ] {
        assert_eq!(
            parse(&config(address.clone(), &[], vec![temp()])),
            Err(vec![refused("modbus.bad-address", 1, &message, fix)]),
            "{address:?}"
        );
    }
}

#[test]
fn refuses_a_host_that_no_resolver_finds() {
    let fix = "Write a host and a port, such as \"plc:502\" or \"[fd00::1]:502\"";
    let part = "has a host with a part between dots that is empty, longer than 63 \
                characters, or starts or ends with `-`";
    let number = "has a host with a number last that is not an IPv4 address";
    let long = format!("{}.local:502", "a".repeat(64));
    for (address, problem) in [
        (".:502", part),
        ("..", part),
        ("a..b:502", part),
        (".plc", part),
        ("plc.", part),
        ("-plc:502", part),
        ("plc-", part),
        (&long, part),
        ("192.168.001.010:502", number),
        ("010.000.000.002", number),
        ("10.1:502", number),
        ("0x0a.0.0.2:502", number),
        ("167772162:502", number),
        ("10.0.0.256", number),
        ("0xa000001:502", number),
        ("10.0xa", number),
        ("10.0XA", number),
        ("plc.0x", number),
    ] {
        let message = format!("the address {address:?} {problem}");
        assert_eq!(
            parse(&config(string(address), &[], vec![temp()])),
            Err(vec![refused("modbus.bad-address", 1, &message, fix)]),
            "{address:?}"
        );
    }
}

#[test]
fn reads_a_part_of_63_characters_and_a_number_that_is_not_last() {
    let long = format!("{}.local", "a".repeat(63));
    for host in [&*long, "10.0.0.2", "1plc", "2.plc", "plc.0xg", "0x1.plc"] {
        let config = parse(&config(string(host), &[], vec![temp()]));
        assert_eq!(config.map(|config| config.host), Ok(host.into()), "{host}");
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
    let fix = "Use `big` when the first register holds the high word, or `little`";
    for (order, message) in [
        (string("middle"), "\"middle\" is not an order"),
        (string("Big"), "\"Big\" is not an order"),
        (
            Value::Bool(true),
            "an order is a string or a reference, not a bool",
        ),
    ] {
        let others = [(16, "order", order.clone())];
        assert_eq!(
            parse(&config(string("plc"), &others, vec![temp()])),
            Err(vec![refused("modbus.bad-order", 17, message, fix)]),
            "{order:?}"
        );
    }
}

#[test]
fn refuses_a_bad_table_or_register_address() {
    let table_fix = "Use `coil`, `discrete`, `holding`, or `input`";
    let address_fix = "Write the address of the protocol, from 0, such as 40 for the \
                       41st holding register";
    for (table, address, refusal) in [
        (
            string("holdings"),
            Value::Integer(40),
            refused(
                "modbus.bad-table",
                31,
                "\"holdings\" is not a table",
                table_fix,
            ),
        ),
        (
            Value::Integer(3),
            Value::Integer(40),
            refused(
                "modbus.bad-table",
                31,
                "a table is a string or a reference, not an integer",
                table_fix,
            ),
        ),
        (
            string("holding"),
            Value::Integer(-1),
            refused(
                "modbus.bad-read-address",
                33,
                "the address -1 is not 0 to 65535",
                address_fix,
            ),
        ),
        (
            string("holding"),
            Value::Integer(65536),
            refused(
                "modbus.bad-read-address",
                33,
                "the address 65536 is not 0 to 65535",
                address_fix,
            ),
        ),
        (
            string("holding"),
            string("40001"),
            refused(
                "modbus.bad-read-address",
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

#[test]
fn refuses_a_read_with_no_channel_or_a_bad_one() {
    let attributes = [
        (30, "table", string("coil")),
        (32, "address", Value::Integer(1)),
    ];
    for (labels, refusal) in [
        (
            &[][..],
            refused(
                "document.label-count",
                20,
                "the `read` block has no labels, and it takes 1 label",
                "Name the channel of the block, such as `read \"plc.temp\"`",
            ),
        ),
        (
            &["Plc Temp"][..],
            refused(
                "document.bad-name",
                21,
                "a segment is not valid: \"Plc Temp\" in \"Plc Temp\"",
                "Use one or more ASCII letters, digits, `_`, and `-` in that segment, \
                 after an optional leading `@`",
            ),
        ),
    ] {
        let reads = vec![block(20, "read", labels, &attributes)];
        assert_eq!(
            parse(&config(string("plc"), &[], reads)),
            Err(vec![refusal]),
            "{labels:?}"
        );
    }
}

#[test]
fn refuses_an_unknown_block_in_the_connector_or_in_a_read() {
    let mut read = temp();
    read.body.blocks.push(block(50, "range", &[], &[]));
    let reads = vec![read, block(40, "write", &["plc.cmd"], &[])];
    assert_eq!(
        parse(&config(string("plc"), &[], reads)),
        Err(vec![
            refused(
                "document.unknown-block",
                50,
                "the `read` block cannot hold the `range` block",
                "Remove it",
            ),
            refused(
                "document.unknown-block",
                40,
                "the connector cannot hold the `write` block",
                "Use `read`, or remove it",
            ),
        ])
    );
}

proptest! {
    #[test]
    fn reads_each_host_and_port(
        name in "([a-zA-Z0-9_]{1,5}\\.){0,3}[a-z_][a-zA-Z0-9_-]{0,8}[a-zA-Z0-9_]",
        ipv4 in any::<std::net::Ipv4Addr>(),
        ipv6 in any::<std::net::Ipv6Addr>(),
        port in 1..=u16::MAX,
    ) {
        let (ipv4, ipv6) = (ipv4.to_string(), ipv6.to_string());
        for (address, want) in [
            (format!("{name}:{port}"), (&*name, port)),
            (name.clone(), (&*name, 502)),
            (format!("{ipv4}:{port}"), (&*ipv4, port)),
            (format!("[{ipv6}]:{port}"), (&*ipv6, port)),
        ] {
            let config = parse(&config(string(&address), &[], vec![temp()]));
            let config = config.expect("a config");
            prop_assert_eq!((&*config.host, config.port), want);
        }
    }
}
