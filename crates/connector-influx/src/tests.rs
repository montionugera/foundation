#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use connector::kind::{Kind as _, Table};
use connector::supervisor::Supervisor;
use connector::testing;
use document::value::Kind as Value;
use document::{Attribute, Block, Map, Position, Source, Span};
use env::tasks::Tasks;

use super::*;

mod send;

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

/// A block with `keyword` at `offset` and `attributes`.
fn block(offset: u32, keyword: &str, attributes: &[(u32, &str, Value)]) -> Block {
    Block {
        keyword: keyword.into(),
        keyword_span: at(offset),
        labels: Vec::new(),
        body: document(attributes, Vec::new()),
        span: at(offset),
    }
}

/// The config of an influx connector, with `address` set to `address`.
fn config(address: Value) -> Document {
    document(
        &[
            (0, "address", address),
            (10, "select", string("edge.*")),
            (14, "database", string("plant")),
            (16, "measurement", string("telemetry")),
        ],
        vec![block(
            20,
            "reader",
            &[(40, "mode", string("complete")), (50, "hold", string("2h"))],
        )],
    )
}

/// The config of an influx connector at `http://influx:8086`.
fn influx() -> Document {
    config(string("http://influx:8086"))
}

fn refused(code: &'static str, at: u32, message: &str, fix: &str) -> Diagnostic {
    Diagnostic::new(Code::new(code), self::at(at), message.into(), fix.into())
}

fn bad_address(message: &str) -> Diagnostic {
    refused(
        "influx.bad-address",
        1,
        message,
        "Remove it, as in \"http://influx:8086\"",
    )
}

#[test]
fn reads_the_address_and_the_reader() {
    let config = config(string("http://influx:8086"));
    let expected = Config {
        address: "http://influx:8086".parse().expect("a URI"),
        database: "plant".into(),
        measurement: "telemetry".into(),
        reader: reader::read(&config, &["address", "database", "measurement"], &[])
            .expect("the reader settings"),
    };
    assert_eq!(Kind.parse(&config), Ok(expected));
}

#[test]
fn checks_to_its_reader_and_no_device_channel() {
    let config = config(string("http://influx:8086"));
    let reader = reader::read(&config, &["address", "database", "measurement"], &[]);
    let table = Table::new().with("influx", Kind);
    assert_eq!(
        table.check("influx", None, &config),
        Ok(Channels {
            reader: Some(reader.expect("the reader settings")),
            ..Channels::default()
        })
    );
}

#[test]
fn reads_an_address_with_a_path_of_slash() {
    let config = config(string("http://influx:8086/"));
    assert_eq!(
        Kind.parse(&config).map(|config| config.address),
        Ok("http://influx:8086/".parse().expect("a URI"))
    );
}

#[test]
fn refuses_an_address_that_uri_refuses() {
    for (address, message) in [
        (
            string("https://influx:8086"),
            "the scheme of the URI is not http",
        ),
        (
            string("http://admin:hunter2@influx:8086"),
            "the URI holds user info; give a credential through a secret",
        ),
        (string("http://[influx]:8086"), "the URI has no valid host"),
        (
            string("http://influx:0"),
            "the port of the URI is not a number from 1 to 65535",
        ),
        (string("influx"), "the scheme of the URI is not http"),
        (
            string("http://influx:8086/#site"),
            "the URI has a fragment, which no request sends",
        ),
        (
            string("http://in flux"),
            "the text is not a URI: invalid uri character",
        ),
        (Value::Integer(8086), "a URI is a string, not an integer"),
    ] {
        assert_eq!(
            Kind.parse(&config(address.clone())),
            Err(vec![refused(
                "connector.bad-uri",
                1,
                message,
                "Write an `http` URI such as \"http://10.0.0.2:8086\"",
            )]),
            "{address:?}"
        );
    }
}

#[test]
fn refuses_an_address_with_a_path_or_a_query() {
    for (address, part) in [
        ("http://influx:8086/api/v2", "a path"),
        ("http://influx:8086//", "a path"),
        ("http://influx:8086/?db=site", "a query"),
        ("http://influx:8086?", "a query"),
    ] {
        assert_eq!(
            Kind.parse(&config(string(address))),
            Err(vec![bad_address(&format!(
                "the address has {part}, which the influx kind does not take"
            ))]),
            "{address}"
        );
    }
}

/// `config` with the value of `key` set to `value`.
fn with(mut config: Document, key: &str, value: &Value) -> Document {
    let attributes = config.attributes.iter().cloned().map(|mut attribute| {
        if *attribute.key == *key {
            attribute.value.kind = value.clone();
        }
        attribute
    });
    config.attributes = Map::new(attributes.collect()).expect("unique keys");
    config
}

#[test]
fn refuses_a_database_that_is_empty_or_not_a_string() {
    for (database, message) in [
        (string(""), "the database name is empty"),
        (Value::Integer(1), "a database is a string, not an integer"),
    ] {
        assert_eq!(
            Kind.parse(&with(influx(), "database", &database)),
            Err(vec![refused(
                "influx.bad-database",
                15,
                message,
                "Write the name of an InfluxDB database, such as \"plant\"",
            )]),
            "{database:?}"
        );
    }
}

#[test]
fn reads_a_database_name_as_it_is_written() {
    let config = with(influx(), "database", &string("plant a/b"));
    assert_eq!(
        Kind.parse(&config).map(|config| config.database),
        Ok("plant a/b".into())
    );
}

#[test]
fn refuses_a_measurement_that_no_line_can_hold() {
    for (measurement, message) in [
        (
            Value::Integer(1),
            "a measurement is a string, not an integer",
        ),
        (
            string("foundation_gaps"),
            "\"foundation_gaps\" is the measurement of the gap lines",
        ),
        (string(""), "the measurement name is empty"),
        (
            string("#plant"),
            "the name \"#plant\" starts with '#', which makes each line a comment",
        ),
    ] {
        assert_eq!(
            Kind.parse(&with(influx(), "measurement", &measurement)),
            Err(vec![refused(
                "influx.bad-measurement",
                17,
                message,
                "Write a measurement name, such as \"telemetry\"",
            )]),
            "{measurement:?}"
        );
    }
}

#[test]
fn refuses_a_config_with_no_address_or_select() {
    let config = document(&[], Vec::new());
    assert_eq!(
        Kind.parse(&config),
        Err(vec![
            Diagnostic::new(
                Code::new("document.missing-attribute"),
                None,
                "the connector has no `select`".into(),
                "Add a `select` attribute with the channels it reads, such as \
                 \"site_a.**\""
                    .into(),
            ),
            Diagnostic::new(
                Code::new("document.missing-attribute"),
                None,
                "the connector has no `address`".into(),
                "Add an `address` attribute with the InfluxDB endpoint, such as \
                 \"http://influx:8086\""
                    .into(),
            ),
            Diagnostic::new(
                Code::new("document.missing-attribute"),
                None,
                "the connector has no `database`".into(),
                "Add a `database` attribute with the InfluxDB database, such as \
                 \"plant\""
                    .into(),
            ),
            Diagnostic::new(
                Code::new("document.missing-attribute"),
                None,
                "the connector has no `measurement`".into(),
                "Add a `measurement` attribute with the name of each line, such as \
                 \"telemetry\""
                    .into(),
            ),
        ])
    );
}

#[test]
fn refuses_each_key_it_does_not_take() {
    let mut config = config(string("http://influx:8086"));
    config.attributes = Map::new(
        config
            .attributes
            .iter()
            .cloned()
            .chain([Attribute {
                key: "token".into(),
                key_span: at(60),
                value: document::value::Value {
                    kind: string("site"),
                    span: at(61),
                },
            }])
            .collect(),
    )
    .expect("unique keys");
    config.blocks.push(block(70, "tls", &[]));
    assert_eq!(
        Kind.parse(&config),
        Err(vec![
            refused(
                "document.unknown-attribute",
                60,
                "`token` is not an attribute of the connector",
                "Use `select`, `address`, `database`, or `measurement`, or remove it",
            ),
            refused(
                "document.unknown-block",
                70,
                "the connector cannot hold the `tls` block",
                "Use `reader`, or remove it",
            ),
        ])
    );
}

#[test]
fn discovers_nothing() {
    let found = run(|_, _| async {
        Table::new()
            .with("influx", Kind)
            .discover("influx", &cancel::Token::new())
            .await
    });
    assert!(matches!(found.as_deref(), Ok([])), "{found:?}");
}

/// Runs `main` on a shard of one simulated node and returns its output.
fn run<T, F>(main: impl FnOnce(::sim::node::Node, Tasks) -> F + Send + 'static) -> T
where
    T: Send + 'static,
    F: Future<Output = T> + 'static,
{
    let mut sim = ::sim::Sim::new(::sim::Config::default());
    let node = sim.node(::sim::node::Config::default());
    sim.run_on(&node, main).expect("the run ends")
}
