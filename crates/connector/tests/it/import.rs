//! Gives the block of a point through `Table::block`, with kinds of this file.

use connector::browse::Facts;
use connector::import::fill;
use connector::kind::{Channels, Context, Error, Kind, Table};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Attribute, Block, Document, Label, Map};
use types::name::Name;

/// A kind whose block of a point is `read "<channel>" { tag = "<tag>", unit = ... }`,
/// with `fill("the unit")` for a unit that the device does not give.
struct Reads;

impl Kind for Reads {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels::default())
    }

    fn run(&self, _: Context<()>) -> impl Future<Output = Result<(), Error>> {
        std::future::ready(Ok(()))
    }

    fn block(
        &self,
        tag: &str,
        facts: &Facts,
        channel: &Name,
    ) -> Result<Document, Error> {
        let unit = facts
            .unit
            .as_deref()
            .map_or_else(|| fill("the unit"), string);
        Ok(read(channel, tag, unit))
    }
}

/// A kind with no block.
struct Blind;

impl Kind for Blind {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels::default())
    }

    fn run(&self, _: Context<()>) -> impl Future<Output = Result<(), Error>> {
        std::future::ready(Ok(()))
    }
}

fn string(text: &str) -> Value {
    Value {
        kind: value::Kind::String(text.into()),
        span: None,
    }
}

/// `read "<channel>" { tag = "<tag>", unit = <unit> }`.
fn read(channel: &Name, tag: &str, unit: Value) -> Document {
    let attribute = |key: &str, value| Attribute {
        key: key.into(),
        key_span: None,
        value,
    };
    let body = Document {
        attributes: Map::new(vec![
            attribute("tag", string(tag)),
            attribute("unit", unit),
        ])
        .expect("distinct keys"),
        blocks: Vec::new(),
    };
    let label = Label {
        text: channel.as_str().into(),
        span: None,
    };
    Document {
        attributes: Map::default(),
        blocks: vec![Block {
            keyword: "read".into(),
            keyword_span: None,
            labels: vec![label],
            body,
            span: None,
        }],
    }
}

fn table() -> Table {
    Table::new().with("blind", Blind).with("reads", Reads)
}

fn channel() -> Name {
    "plc.Line1.Temp".parse().expect("a valid name")
}

/// The one diagnostic of a config error.
fn diagnostic(result: Result<Document, Error>) -> Diagnostic {
    match result {
        Err(Error::Config(mut diagnostics)) if diagnostics.len() == 1 => {
            diagnostics.remove(0)
        }
        other => panic!("not one config diagnostic: {other:?}"),
    }
}

#[test]
fn gives_the_block_of_the_kind() {
    let facts = Facts {
        unit: Some("degC".into()),
        ..Facts::default()
    };
    let block = table().block("reads", "ns=2;s=T1", &facts, &channel());
    assert_eq!(
        block.ok(),
        Some(read(&channel(), "ns=2;s=T1", string("degC")))
    );
}

#[test]
fn fills_a_fact_that_the_device_does_not_give() {
    let block = table().block("reads", "ns=2;s=T1", &Facts::default(), &channel());
    assert_eq!(
        block.ok(),
        Some(read(&channel(), "ns=2;s=T1", fill("the unit")))
    );
}

#[test]
fn refuses_a_kind_that_cannot_write_a_block() {
    let result = table().block("blind", "ns=2;s=T1", &Facts::default(), &channel());
    let expected = Diagnostic::new(
        Code::new("connector.no-import"),
        None,
        "this connector kind cannot write the block of a point".into(),
        "Write the blocks of the points by hand".into(),
    );
    assert_eq!(diagnostic(result), expected);
}

#[test]
fn refuses_a_kind_that_the_build_does_not_have() {
    let result = table().block("modbus", "ns=2;s=T1", &Facts::default(), &channel());
    let expected = Diagnostic::new(
        Code::new("connector.unknown-kind"),
        None,
        r#"this build has no connector kind "modbus""#.into(),
        r#"Use one of ["blind", "reads"]"#.into(),
    );
    assert_eq!(diagnostic(result), expected);
}
