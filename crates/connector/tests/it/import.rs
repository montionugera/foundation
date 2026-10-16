//! Gives the blocks of a point through `Table::block`, with kinds of this file.

use connector::browse::{Access, Facts};
use connector::kind::{Channels, Context, Error, Kind, Table};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Attribute, Block, Document, Label, Map};
use types::name::Name;

/// A kind whose blocks of a point are `read "<channel>" { tag = "<tag>" }`, a `write`
/// block in its place for a point that a connector can only write, and both for a
/// point that it can read and write.
struct Points;

impl Kind for Points {
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
    ) -> Result<Vec<Block>, Vec<Diagnostic>> {
        let keywords: &[&str] = match facts.access {
            None | Some(Access::Read) => &["read"],
            Some(Access::Write) => &["write"],
            Some(Access::ReadWrite) => &["read", "write"],
        };
        Ok(keywords
            .iter()
            .map(|keyword| point(keyword, channel, tag))
            .collect())
    }
}

/// A kind with no blocks.
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

/// `<keyword> "<channel>" { tag = "<tag>" }`.
fn point(keyword: &str, channel: &Name, tag: &str) -> Block {
    let tag = Attribute {
        key: "tag".into(),
        key_span: None,
        value: Value {
            kind: value::Kind::String(tag.into()),
            span: None,
        },
    };
    let label = Label {
        text: channel.as_str().into(),
        span: None,
    };
    Block {
        keyword: keyword.into(),
        keyword_span: None,
        labels: vec![label],
        body: Document {
            attributes: Map::new(vec![tag]).expect("one key"),
            blocks: Vec::new(),
        },
        span: None,
    }
}

fn table() -> Table {
    Table::new().with("blind", Blind).with("points", Points)
}

fn channel() -> Name {
    "plc.Line1.Temp".parse().expect("a valid name")
}

fn facts(access: Option<Access>) -> Facts {
    Facts {
        access,
        ..Facts::default()
    }
}

#[test]
fn gives_the_blocks_of_the_kind() {
    let blocks = table().block("points", "ns=2;s=T1", &facts(None), &channel());
    assert_eq!(blocks, Ok(vec![point("read", &channel(), "ns=2;s=T1")]));
}

#[test]
fn gives_each_block_that_the_kind_gives_for_a_point() {
    let access = Some(Access::ReadWrite);
    let blocks = table().block("points", "ns=2;s=T1", &facts(access), &channel());
    let expected = vec![
        point("read", &channel(), "ns=2;s=T1"),
        point("write", &channel(), "ns=2;s=T1"),
    ];
    assert_eq!(blocks, Ok(expected));
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
    assert_eq!(result, Err(vec![expected]));
}

#[test]
fn refuses_a_kind_that_the_build_does_not_have() {
    let result = table().block("modbus", "ns=2;s=T1", &Facts::default(), &channel());
    let expected = Diagnostic::new(
        Code::new("connector.unknown-kind"),
        None,
        r#"this build has no connector kind "modbus""#.into(),
        r#"Use one of ["blind", "points"]"#.into(),
    );
    assert_eq!(result, Err(vec![expected]));
}
