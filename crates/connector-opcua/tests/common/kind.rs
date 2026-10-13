//! A connector of the `opcua` kind that reads Double variables of the sim server on
//! one sim node, with a measure of each poll of its run, and one that writes the
//! frames of its reads with no server, with a measure of each write. For the `alloc`
//! test and the `kind` bench.

use std::future::{Future as _, poll_fn};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use connector::cancel::Token;
use connector::kind::{self, Channels, Context, Error, Table};
use connector::supervisor::{self, Supervisor};
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

/// What a run of [`Measured`] measures, and where it puts what the measure gives.
enum Of {
    /// Each poll of the run of the `opcua` kind.
    Polls(Polls),
    /// Each of `count` writes of a frame of its reads, with no server.
    Writes {
        count: usize,
        measures: Arc<Mutex<Vec<u64>>>,
    },
}

/// The `opcua` kind, with `measure` around each poll of its run or each write.
struct Measured<M> {
    measure: M,
    of: Of,
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
        match &self.of {
            Of::Polls(polls) => {
                let kind = connector_opcua::Kind::default();
                let clock = ctx.clock().clone();
                let mut run = pin!(kind.run(ctx));
                poll_fn(|cx| {
                    let mut poll = Poll::Pending;
                    let value = (self.measure)(&mut || poll = run.as_mut().poll(cx));
                    let mut polls = polls.lock().expect("no panic under the lock");
                    polls.push((clock.now(), value));
                    poll
                })
                .await
            }
            Of::Writes { count, measures } => {
                let writes = connector_opcua::bench::kind::writes;
                let written = writes(&ctx, *count, &self.measure).await?;
                *measures.lock().expect("no panic under the lock") = written;
                Ok(())
            }
        }
    }
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// Runs a connector at 10 Hz that reads `nodes` Double variables of the sim server,
/// for `count` ticks and two more, and gives what `measure` gives for its polls: for
/// the run start (the open of its writer, its frames, its connect, and its first
/// request), and for each of the first `count` reads. `measure` calls the poll that it
/// gets once.
///
/// # Panics
///
/// If the run ends with an error, or the server answered fewer than `count` reads.
pub(crate) fn run(
    nodes: usize,
    count: usize,
    measure: impl Fn(&mut dyn FnMut()) -> u64 + Send + Sync + 'static,
) -> Measures {
    let polls = Polls::default();
    let kind = Measured {
        measure,
        of: Of::Polls(Arc::clone(&polls)),
    };
    let texts: Vec<String> = (2..).take(nodes).map(|key| read(key).1).collect();
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = sim.run_on(&node, move |node, tasks| async move {
        let inputs = inputs(&node, &tasks, kind, nodes).await;
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
        let config = config(&address, nodes);
        let (token, clock) = (Token::new(), node.clock());
        let canceller = token.clone();
        let ticks = i64::try_from(count + 2).expect("a count");
        tasks.spawn(async move {
            clock.sleep(Span::from_nanos(PERIOD.nanos() * ticks)).await;
            canceller.cancel();
        });
        let supervisor = Supervisor::new(inputs);
        let open = supervisor.open("opcua", name("plant.plc"), &config, &token);
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

/// Writes `count` frames of a connector that reads `nodes` Double variables, with no
/// server, and gives what `measure` gives for the check of the values and the write
/// of each frame. `measure` calls the work that it gets once.
///
/// # Panics
///
/// If the run ends with an error.
pub(crate) fn writes(
    nodes: usize,
    count: usize,
    measure: impl Fn(&mut dyn FnMut()) -> u64 + Send + Sync + 'static,
) -> Vec<u64> {
    let measures = Arc::<Mutex<Vec<u64>>>::default();
    let kind = Measured {
        measure,
        of: Of::Writes {
            count,
            measures: Arc::clone(&measures),
        },
    };
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = sim.run_on(&node, move |node, tasks| async move {
        let supervisor = Supervisor::new(inputs(&node, &tasks, kind, nodes).await);
        let config = config("opc.tcp://plc:4840", nodes);
        let token = Token::new();
        let open = supervisor.open("opcua", name("plant.plc"), &config, &token);
        open.await.run().await
    });
    let result = run.expect("the run ends");
    assert!(result.is_ok(), "the run ends ok: {result:?}");
    let measures = measures.lock().expect("no panic under the lock");
    measures.clone()
}

/// The inputs of a supervisor on `node` with `kind` as `opcua`, and the channels of
/// `plant.plc`, a connector that reads `nodes` nodes: its status, the index
/// `plant.time` of key 1, and an `f64` channel of key 2 on for each read.
async fn inputs(
    node: &sim::node::Node,
    tasks: &env::tasks::Tasks,
    kind: impl kind::Kind,
    nodes: usize,
) -> supervisor::Config {
    let env = hub::testing::Env {
        files: node.files(),
        clock: node.clock(),
        wall: node.wall(),
        entropy: node.entropy(),
        tasks: tasks.clone(),
    };
    let kinds = Table::new().with("opcua", kind);
    let inputs = testing::create_config(env, node.net(), kinds).await;
    let counts = ["in", "missed", "lost"].map(name);
    let first = channel::Key::from_u128(1_000_000);
    let mut definitions = testing::create_status(&name("plant.plc"), &counts, first);
    definitions.push((name("plant.time"), index(1)));
    let reads = (2..).take(nodes).map(|key| (name(&read(key).0), data(key)));
    definitions.extend(reads);
    inputs
        .hub
        .set_definitions(definitions.iter().map(|(name, def)| (name, def)));
    inputs
}

/// What a measure gives for the polls of a run of the `opcua` kind.
pub(crate) struct Measures {
    /// The sum for the polls before the first answer of the server.
    pub(crate) start: u64,
    /// For each read, the sum for the polls within half a period of its answer.
    pub(crate) ticks: Vec<u64>,
}

/// The sums of the values of `polls`: before the first of `served`, and within half
/// a period of each of `served`.
fn sums(polls: &[(Monotonic, u64)], served: &[Served]) -> Measures {
    let half = PERIOD.nanos() / 2;
    let near = |at: Monotonic, read: Monotonic| (at - read).nanos().abs() < half;
    let start = polls.iter().take_while(|(at, _)| *at < served[0].time);
    let ticks = (served.iter())
        .map(|read| {
            let polls = polls.iter().filter(|(at, _)| near(*at, read.time));
            polls.map(|(_, value)| value).sum()
        })
        .collect();
    Measures {
        start: start.map(|(_, value)| value).sum(),
        ticks,
    }
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

/// The channel of `key` and the node that it reads: `plant.v<key>` and
/// `ns=2;i=<key - 1>`.
fn read(key: u128) -> (String, String) {
    (format!("plant.v{key}"), format!("ns=2;i={}", key - 1))
}

/// The config of a connector of `address` at 10 Hz that reads `nodes` nodes, from
/// `ns=2;i=1` into the channel of key 2 on.
pub(crate) fn config(address: &str, nodes: usize) -> Document {
    let text = |text: &str| value::Kind::String(text.into());
    let attributes = vec![
        attribute("address", text(address)),
        attribute("rate", text("10Hz")),
    ];
    let blocks = (2..).take(nodes).map(read).map(|(channel, node)| Block {
        keyword: "read".into(),
        keyword_span: None,
        labels: vec![document::Label {
            text: channel.into(),
            span: None,
        }],
        body: Document {
            attributes: Map::new(vec![attribute("node_id", text(&node))])
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
