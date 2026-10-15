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
        target: Target::Database("plant".into()),
        measurement: "telemetry".into(),
        token: None,
        reader: reader::read(&config, &KEYS, &[]).expect("the reader settings"),
    };
    assert_eq!(Kind.parse(&config), Ok(expected));
}

/// A call of `function` with the string `argument`.
fn call(function: &str, argument: &str) -> Value {
    Value::Call(document::value::Call {
        function: function.into(),
        function_span: at(61),
        arguments: vec![document::value::Value {
            kind: string(argument),
            span: at(62),
        }],
    })
}

#[test]
fn reads_the_source_of_a_token() {
    for (value, source) in [
        (
            call("env", "INFLUX_TOKEN"),
            secret::Source::Env("INFLUX_TOKEN".into()),
        ),
        (
            call("secret", "plant.influx_token"),
            secret::Source::Secret("plant.influx_token".parse().expect("a name")),
        ),
    ] {
        let config = plus(influx(), &[(60, "token", value)]);
        assert_eq!(
            Kind.parse(&config).map(|config| config.token),
            Ok(Some(source))
        );
    }
}

#[test]
fn refuses_a_token_that_is_not_a_source() {
    let config = plus(influx(), &[(60, "token", string("my-t0ken"))]);
    assert_eq!(
        Kind.parse(&config),
        Err(vec![refused(
            "secret.source",
            61,
            "a secret takes where its value comes from, never the value",
            "Write `env(\"NAME\")`, `file(\"/absolute/path\")`, or `secret(\"name\")`",
        )])
    );
}

#[test]
fn checks_to_its_reader_and_the_count_confirmed_and_no_device_channel() {
    let config = config(string("http://influx:8086"));
    let reader = reader::read(&config, &KEYS, &[]);
    let table = Table::new().with("influx", Kind);
    assert_eq!(
        table.check("influx", None, &config),
        Ok(Channels {
            reader: Some(reader.expect("the reader settings")),
            writes: Vec::new(),
            counts: vec!["confirmed".parse().expect("a name")],
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
        (Value::Integer(1), "`database` is a string, not an integer"),
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
fn refuses_a_name_over_4096_bytes() {
    for (over, bytes) in [("a".repeat(4_097), 4_097), ("é".repeat(2_049), 4_098)] {
        for (config, key, code, offset) in [
            (influx(), "database", "influx.bad-database", 15),
            (bucket(), "bucket", "influx.bad-bucket", 61),
            (bucket(), "org", "influx.bad-org", 63),
        ] {
            assert_eq!(
                Kind.parse(&with(config, key, &string(&over))),
                Err(vec![refused(
                    code,
                    offset,
                    &format!("the {key} name has {bytes} bytes, over 4096"),
                    &format!("Write the name of an InfluxDB {key}, such as \"plant\""),
                )]),
            );
        }
    }
    let longest = string(&"a".repeat(4_096));
    let parsed = Kind.parse(&with(influx(), "database", &longest));
    assert_eq!(
        parsed.map(|config| config.target),
        Ok(Target::Database("a".repeat(4_096)))
    );
}

#[test]
fn reads_a_database_name_as_it_is_written() {
    let config = with(influx(), "database", &string("plant a/b"));
    assert_eq!(
        Kind.parse(&config).map(|config| config.target),
        Ok(Target::Database("plant a/b".into()))
    );
}

/// `config` with `attributes` added, each as [`document`] places it.
fn plus(mut config: Document, attributes: &[(u32, &str, Value)]) -> Document {
    let added = document(attributes, Vec::new()).attributes;
    let attributes = config.attributes.iter().chain(added.iter()).cloned();
    config.attributes = Map::new(attributes.collect()).expect("unique keys");
    config
}

/// `config` with no `key`.
fn without(mut config: Document, key: &str) -> Document {
    let attributes = config
        .attributes
        .iter()
        .filter(|attribute| *attribute.key != *key);
    config.attributes = Map::new(attributes.cloned().collect()).expect("unique keys");
    config
}

/// The config of an influx connector that writes to the bucket `edge` in `acme`.
fn bucket() -> Document {
    plus(
        without(influx(), "database"),
        &[(60, "bucket", string("edge")), (62, "org", string("acme"))],
    )
}

#[test]
fn reads_a_bucket_and_an_org() {
    assert_eq!(
        Kind.parse(&bucket()).map(|config| config.target),
        Ok(Target::Bucket {
            name: "edge".into(),
            org: "acme".into()
        })
    );
}

#[test]
fn refuses_a_database_and_a_bucket() {
    let config = plus(influx(), &[(60, "bucket", string("edge"))]);
    assert_eq!(
        Kind.parse(&config),
        Err(vec![refused(
            "influx.two-targets",
            60,
            "the connector has a `database` and a `bucket`",
            "Keep `database` for InfluxDB 1, or `bucket` and `org` for InfluxDB 2 \
             and 3",
        )])
    );
}

#[test]
fn refuses_an_org_with_a_database() {
    let config = plus(influx(), &[(62, "org", string("acme"))]);
    assert_eq!(
        Kind.parse(&config),
        Err(vec![refused(
            "influx.org-with-database",
            62,
            "the connector has an `org`, which only a `bucket` takes",
            "Remove it, or write a `bucket` in place of the `database`",
        )])
    );
}

#[test]
fn refuses_a_bucket_with_no_org() {
    assert_eq!(
        Kind.parse(&without(bucket(), "org")),
        Err(vec![Diagnostic::new(
            Code::new("document.missing-attribute"),
            None,
            "the connector has no `org`".into(),
            "Add an `org` attribute with the InfluxDB organization, such as \"plant\". \
             InfluxDB 3 takes any name"
                .into(),
        )])
    );
}

#[test]
fn refuses_an_org_with_no_database_or_bucket() {
    let config = plus(
        without(influx(), "database"),
        &[(62, "org", string("acme"))],
    );
    assert_eq!(
        Kind.parse(&config),
        Err(vec![Diagnostic::new(
            Code::new("document.missing-attribute"),
            None,
            "the connector has no `database` or `bucket`".into(),
            "Add a `bucket` attribute and an `org` attribute, such as \"plant\", or a \
             `database` attribute for InfluxDB 1"
                .into(),
        )])
    );
}

#[test]
fn refuses_a_bucket_and_an_org_that_are_empty_or_not_strings() {
    let config = with(bucket(), "bucket", &string(""));
    let config = with(config, "org", &Value::Integer(1));
    assert_eq!(
        Kind.parse(&config),
        Err(vec![
            refused(
                "influx.bad-bucket",
                61,
                "the bucket name is empty",
                "Write the name of an InfluxDB bucket, such as \"plant\"",
            ),
            refused(
                "influx.bad-org",
                63,
                "`org` is a string, not an integer",
                "Write the name of an InfluxDB org, such as \"plant\"",
            ),
        ])
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
                "the connector has no `database` or `bucket`".into(),
                "Add a `bucket` attribute and an `org` attribute, such as \"plant\", \
                 or a `database` attribute for InfluxDB 1"
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
    let mut config = plus(influx(), &[(60, "user", string("site"))]);
    config.blocks.push(block(70, "tls", &[]));
    assert_eq!(
        Kind.parse(&config),
        Err(vec![
            refused(
                "document.unknown-attribute",
                60,
                "`user` is not an attribute of the connector",
                "Use `select`, `address`, `database`, `bucket`, `org`, `measurement`, \
                 or `token`, or remove it",
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
