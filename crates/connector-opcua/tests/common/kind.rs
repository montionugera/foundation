//! A connector of the `opcua` kind that reads Double variables of the sim server on
//! one sim node, with a measure of each poll of its run, for the `alloc` test and the
//! `kind` bench.

use std::future::{Future as _, poll_fn};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use connector::cancel::Token;
use connector::kind::{self, Channels, Context, Error, Table};
use connector::supervisor::Supervisor;
use connector::testing;
use connector_opcua::Config;
use connector_opcua::sim::{Served, Variables, serve};
use document::Document;
use document::diagnostic::Diagnostic;
use document::value::{self, Value};
use document::{Attribute, Block, Map};
use env::net::tcp;
use env::tasks::Group;
use spec::channel::{Channel, Data};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::channel;
use types::name::Name;
use types::sample::{Scalar, Type};
use types::time::{Monotonic, Span};

/// The time between two ticks of the connector, at 10 Hz.
const PERIOD: Span = Span::from_nanos(100_000_000);

/// What a measure gives for each poll of a run: the time of the poll and the value.
type Polls = Arc<Mutex<Vec<(Monotonic, u64)>>>;

/// The `opcua` kind, with `measure` around each poll of its run.
struct Measured<M> {
    measure: M,
    polls: Polls,
}

impl<M: Fn(&mut dyn FnMut()) -> u64 + Send + Sync + 'static> kind::Kind
    for Measured<M>
{
    type Config = Config;

    fn parse(&self, config: &Document) -> Result<Config, Vec<Diagnostic>> {
        connector_opcua::Kind::default().parse(config)
    }

    fn check(&self, config: &Config) -> Result<Channels, Vec<Diagnostic>> {
        connector_opcua::Kind::default().check(config)
    }

    fn discover(
        &self,
        _: &Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn run(&self, ctx: Context<Config>) -> Result<(), Error> {
        let (kind, clock) = (connector_opcua::Kind::default(), ctx.clock().clone());
        let mut run = pin!(kind.run(ctx));
        poll_fn(|cx| {
            let mut poll = Poll::Pending;
            let value = (self.measure)(&mut || poll = run.as_mut().poll(cx));
            let mut polls = self.polls.lock().expect("no panic under the lock");
            polls.push((clock.now(), value));
            poll
        })
        .await
    }
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// Runs a connector at 10 Hz that reads `nodes` Double variables of the sim server,
/// for `count` ticks and two more. Gives the sum of what `measure` gives for the polls
/// of the run before the first answer of the server (the run start: the open of its
/// writer, its frames, its connect, and its first request), and, for each of the first
/// `count` reads, the sum for the polls within half a period of the answer of the
/// server. `measure` calls the poll that it gets once.
///
/// # Panics
///
/// If the run ends with an error, or the server answered fewer than `count` reads.
pub(crate) fn ticks(
    nodes: usize,
    count: usize,
    measure: impl Fn(&mut dyn FnMut()) -> u64 + Send + Sync + 'static,
) -> (u64, Vec<u64>) {
    let polls = Polls::default();
    let kind = Measured {
        measure,
        polls: Arc::clone(&polls),
    };
    let texts: Vec<String> = (1..=nodes).map(|n| format!("ns=2;i={n}")).collect();
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = sim.run_on(&node, move |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let kinds = Table::new().with("opcua", kind);
        let inputs = testing::create_config(env, node.net(), kinds).await;
        let plc = name("plant.plc");
        let counts = ["in", "missed", "lost"].map(name);
        let first = channel::Key::from_u128(1_000_000);
        let mut definitions = testing::create_status(&plc, &counts, first);
        definitions.push((name("plant.time"), index(1)));
        let channels = (2..).take(nodes).map(|key| (format!("plant.v{key}"), key));
        let channels: Vec<_> = channels.collect();
        definitions.extend(channels.iter().map(|(text, key)| (name(text), data(*key))));
        inputs
            .hub
            .set_definitions(definitions.iter().map(|(name, def)| (name, def)));
        let variables =
            Variables::new(&texts.iter().map(String::as_str).collect::<Vec<_>>());
        let (stop, serving) = (Token::new(), Group::new(tasks.clone()));
        let (clock, net, listener) = (node.clock(), node.net(), listen(&node));
        let (answers, until) = (variables.clone(), stop.clone());
        let mut rng = node.entropy().rng();
        serving.tasks().spawn(async move {
            serve(clock, net, listener, &mut rng, &answers, until.wait()).await;
        });
        let address = format!("opc.tcp://{}:4840", node.addresses()[0]);
        let reads = channels.iter().zip(&texts);
        let reads = reads.map(|((channel, _), node)| (channel.as_str(), node.as_str()));
        let config = config(&address, reads);
        let (token, clock) = (Token::new(), node.clock());
        let canceller = token.clone();
        let ticks = i64::try_from(count + 2).expect("a count");
        tasks.spawn(async move {
            clock.sleep(Span::from_nanos(PERIOD.nanos() * ticks)).await;
            canceller.cancel();
        });
        let supervisor = Supervisor::new(inputs);
        let open = supervisor.open("opcua", plc, &config, &token);
        let result = open.await.run().await;
        stop.cancel();
        serving.ended().await;
        let mut served = variables.served();
        (result, served.remove(&texts[0]).expect("the first node"))
    });
    let (result, served) = run.expect("the run ends");
    assert!(result.is_ok(), "the run ends ok: {result:?}");
    assert!(served.len() >= count, "{} reads served", served.len());
    sums(
        &polls.lock().expect("no panic under the lock"),
        &served[..count],
    )
}

/// The sum of the values of `polls` before the first of `served`, and for each of
/// `served`, the sum of those within half a period of it.
fn sums(polls: &[(Monotonic, u64)], served: &[Served]) -> (u64, Vec<u64>) {
    let half = PERIOD.nanos() / 2;
    let near = |at: Monotonic, read: Monotonic| (at - read).nanos().abs() < half;
    let start = polls.iter().take_while(|(at, _)| *at < served[0].time);
    let ticks = (served.iter())
        .map(|read| {
            let polls = polls.iter().filter(|(at, _)| near(*at, read.time));
            polls.map(|(_, value)| value).sum()
        })
        .collect();
    (start.map(|(_, value)| value).sum(), ticks)
}

/// The definition of an index of `key`.
fn index(key: u128) -> Definition {
    Definition::Channel(Channel {
        key: channel::Key::from_u128(key),
        kind: spec::channel::Kind::Index {
            error: None,
            control: None,
        },
    })
}

/// The definition of an `f64` channel of `key` on the index of key 1.
fn data(key: u128) -> Definition {
    let index = channel::Key::from_u128(1);
    let sample = DataType::Sample(Type::Scalar(Scalar::F64));
    let data = Data::new(index, None, sample, None).expect("no unit");
    Definition::Channel(Channel {
        key: channel::Key::from_u128(key),
        kind: spec::channel::Kind::Data(data),
    })
}

/// An attribute of `key` and `kind`, with no span.
fn attribute(key: &str, kind: value::Kind) -> Attribute {
    Attribute {
        key: key.into(),
        key_span: None,
        value: Value { kind, span: None },
    }
}

/// The config of a connector of `address` at 10 Hz with a `read` block of each
/// channel and node of `reads`.
pub(crate) fn config<'a>(
    address: &str,
    reads: impl Iterator<Item = (&'a str, &'a str)>,
) -> Document {
    let text = |text: &str| value::Kind::String(text.into());
    let attributes = vec![
        attribute("address", text(address)),
        attribute("rate", text("10Hz")),
    ];
    let blocks = reads.map(|(channel, node)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![document::Label {
            text: channel.into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![attribute("node_id", text(node))])
                .expect("one key"),
            blocks: Vec::new(),
        },
        span: None,
    });
    Document {
        attributes: Map::new(attributes).expect("unique keys"),
        blocks: blocks.collect(),
    }
}

/// Binds a listener of `node` on its port 4840.
fn listen(node: &sim::node::Node) -> env::net::Listener {
    let listen = tcp::Listen {
        local: SocketAddr::new(node.addresses()[0], 4840),
        backlog: 4,
        options: tcp::Options {
            send_buffer_bytes: 1 << 16,
            recv_buffer_bytes: 1 << 16,
            unsent_bytes_max: NonZeroUsize::new(1 << 14).expect("not 0"),
            delayed: false,
        },
    };
    node.net().listen(&listen).expect("the port is free")
}
