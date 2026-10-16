//! Browses through `Supervisor::browse`, with kinds of this file.

use connector::browse::{
    self, Child, Cursor, Device, End, Entry, Facts, Page, Path, Query, Scope, Shape,
    Values,
};
use connector::cancel;
use connector::kind::{Channels, Context, Error, Kind, Table};
use connector::secret::Source;
use connector::supervisor::Supervisor;
use connector::testing::{create_config, create_secret};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Attribute, Document, Map};

/// A kind whose device at `address` holds the folder `Line1` with the point `Temp1`,
/// with the address in its tag. It reads the secret `plant.token`, and its device
/// refuses a token other than `t1`. Its `parse` needs `rate`, which a browse does not.
struct Plant;

impl Kind for Plant {
    type Config = ();

    fn parse(&self, config: &Document) -> Result<(), Vec<Diagnostic>> {
        match config.attributes.get("rate") {
            Some(_) => Ok(()),
            None => Err(vec![Diagnostic::new(
                Code::new("plant.no-rate"),
                None,
                "the block has no rate".into(),
                "Give it a rate".into(),
            )]),
        }
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels::default())
    }

    async fn discover(&self, _: &cancel::Token) -> Result<Vec<Document>, Error> {
        Ok(Vec::new())
    }

    async fn run(&self, _: Context<()>) -> Result<(), Error> {
        Ok(())
    }

    async fn browse(
        &self,
        ctx: &browse::Context<'_>,
        query: &Query,
    ) -> Result<browse::Found, Error> {
        let address = match config(ctx.config(), "address") {
            Some(address) => address.to_owned(),
            None => return Err(Error::Device("no address".into())),
        };
        let source = Source::Secret("plant.token".parse().unwrap());
        if ctx.secret(&source).await?.expose() != b"t1" {
            return Err(Error::Device("the device refused the token".into()));
        }
        browse::walk(&mut Session { address }, query).await
    }
}

/// The text of the attribute `key` of `config`.
fn config<'a>(config: &'a Document, key: &str) -> Option<&'a str> {
    match &config.attributes.get(key)?.value.kind {
        value::Kind::String(text) => Some(text),
        _ => None,
    }
}

/// The open session of a [`Plant`] device.
struct Session {
    address: String,
}

impl Device for Session {
    async fn level(
        &mut self,
        folder: Option<&Entry>,
        _: Option<&Cursor>,
    ) -> Result<Option<Page>, Error> {
        let child = |name: &str, shape| Child {
            name: name.into(),
            tag: format!("{}/{name}", self.address),
            shape,
        };
        let entries = match folder.map(|folder| folder.tag.as_str()) {
            None => vec![child("Line1", Shape::Folder)],
            Some(_) => vec![child("Temp1", Shape::Point(Facts::default()))],
        };
        Ok(Some(Page {
            entries,
            next: None,
        }))
    }
}

/// A kind with no browse.
struct Blind;

impl Kind for Blind {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels::default())
    }

    async fn discover(&self, _: &cancel::Token) -> Result<Vec<Document>, Error> {
        Ok(Vec::new())
    }

    async fn run(&self, _: Context<()>) -> Result<(), Error> {
        Ok(())
    }
}

/// A connector block with only `kind`, `node`, and `address`.
fn block(kind: &str) -> Document {
    let text = |key: &str, text: &str| Attribute {
        key: key.into(),
        key_span: None,
        value: Value {
            kind: value::Kind::String(text.into()),
            span: None,
        },
    };
    let attributes = vec![
        text("kind", kind),
        text("node", "n1"),
        text("address", "opc.tcp://plc:4840"),
    ];
    Document {
        attributes: Map::new(attributes).expect("distinct keys"),
        blocks: Vec::new(),
    }
}

/// What `Supervisor::browse` gives for `kind` and `query` over [`block`], with the
/// secret `plant.token` set to `token`.
fn browse(kind: &str, query: &Query, token: &[u8]) -> Result<browse::Found, Error> {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let (kind, query, token) = (kind.to_owned(), query.clone(), token.to_vec());
    let result = sim.run_on(&node, move |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks,
        };
        let kinds = Table::new().with("plant", Plant).with("blind", Blind);
        let config = create_config(env, node.net(), kinds).await;
        create_secret(&node.files(), &"plant.token".parse().unwrap(), &token).await;
        let supervisor = Supervisor::new(config);
        supervisor.browse(&kind, &block(&kind), &query).await
    });
    result.expect("the run ends")
}

fn query(scope: Scope) -> Query {
    Query {
        start: Path::root(),
        scope,
        values: Values::Skipped,
    }
}

/// The one diagnostic of `result`, which is [`Error::Config`].
fn diagnostic(result: Result<browse::Found, Error>) -> Diagnostic {
    match result {
        Err(Error::Config(mut diagnostics)) if diagnostics.len() == 1 => {
            diagnostics.remove(0)
        }
        other => panic!("not one config diagnostic: {other:?}"),
    }
}

#[test]
fn browses_a_block_with_only_its_kind_node_and_address() {
    let pattern = "**".parse().expect("a pattern");
    let found = browse("plant", &query(Scope::Match(pattern)), b"t1").expect("found");
    let entries: Vec<_> = (found.entries.iter())
        .map(|entry| (entry.path.to_string(), entry.tag.as_str()))
        .collect();
    assert_eq!(
        entries,
        [("Line1/Temp1".to_owned(), "opc.tcp://plc:4840/Temp1")]
    );
    assert_eq!(found.end, End::Done);
}

#[test]
fn gives_the_error_of_the_kind() {
    let result = browse("plant", &query(Scope::Level(None)), b"t2");
    let Err(Error::Device(error)) = result else {
        panic!("not a device error: {result:?}");
    };
    assert_eq!(error.to_string(), "the device refused the token");
}

#[test]
fn refuses_a_kind_that_cannot_browse() {
    let result = browse("blind", &query(Scope::Level(None)), b"t1");
    let expected = Diagnostic::new(
        Code::new("connector.no-browse"),
        None,
        "this connector kind cannot browse its device".into(),
        "Write the blocks of the points by hand".into(),
    );
    assert_eq!(diagnostic(result), expected);
}

#[test]
fn refuses_a_kind_that_the_build_does_not_have() {
    let result = browse("modbus", &query(Scope::Level(None)), b"t1");
    let expected = Diagnostic::new(
        Code::new("connector.unknown-kind"),
        None,
        r#"this build has no connector kind "modbus""#.into(),
        r#"Use one of ["blind", "plant"]"#.into(),
    );
    assert_eq!(diagnostic(result), expected);
}
