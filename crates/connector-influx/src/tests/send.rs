use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::rc::Rc;

use bytes::Bytes;
use connector::supervisor;
use env::clock::Clock;
use env::net::tcp;
use http::{Request, Response, StatusCode};
use hub::reader::Mode;
use spec::channel::{Channel, Data};
use spec::data_type::DataType;
use spec::definition::Definition;
use types::authority::Authority;
use types::channel;
use types::frame::{Form, Label, Path};
use types::name::{Name, Selector};
use types::sample::{Scalar, Type};
use types::time::Span;

use super::*;

const PORT: u16 = 8086;
const OPTIONS: tcp::Options = tcp::Options {
    send_buffer_bytes: 1 << 16,
    recv_buffer_bytes: 1 << 16,
    unsent_bytes_max: NonZeroUsize::new(1 << 14).unwrap(),
    delayed: false,
};
/// The name, key, and type of each channel of another sample type, on `edge.time`.
const TYPED: [(&str, u128, Type); 15] = [
    ("edge.bool", 10, Type::Scalar(Scalar::Bool)),
    ("edge.i8", 11, Type::Scalar(Scalar::I8)),
    ("edge.i16", 12, Type::Scalar(Scalar::I16)),
    ("edge.i32", 13, Type::Scalar(Scalar::I32)),
    ("edge.u8", 14, Type::Scalar(Scalar::U8)),
    ("edge.u16", 15, Type::Scalar(Scalar::U16)),
    ("edge.u32", 16, Type::Scalar(Scalar::U32)),
    ("edge.u64", 17, Type::Scalar(Scalar::U64)),
    ("edge.f32", 18, Type::Scalar(Scalar::F32)),
    ("edge.stamp", 19, Type::Scalar(Scalar::Stamp)),
    ("edge.span", 20, Type::Scalar(Scalar::Span)),
    ("edge.uuid", 21, Type::Scalar(Scalar::Uuid)),
    (
        "edge.xyz",
        22,
        Type::Array {
            element: Scalar::F32,
            len: 3,
        },
    ),
    ("edge.text", 23, Type::String),
    ("edge.blob", 24, Type::Bytes),
];
/// The key of the first status channel of `influx`.
const STATUS: u128 = 100;
/// The file of the token of `file(TOKEN_FILE)`.
const TOKEN_FILE: &str = "/run/secrets/influx";
/// The value of `connector::status::Class::Retry` on the `class` channel.
const RETRY: u8 = 3;

/// One request that the InfluxDB of a plant got: its path and query, and its body.
type Got = (String, String);

/// The end of one supervised run, once it returned.
type End = Rc<RefCell<Option<Result<(), Error>>>>;

/// A node with a hub that defines `edge.time` (key 1), the `f64` channels `edge.a`
/// (2) and `edge.b` (3), the `i64` channel `edge.count` (4), the `f64` channel `time`
/// (5), a channel of each type of [`TYPED`], and the status channels of `influx`, and
/// an InfluxDB that answers each request with the next answer of its list. After the
/// last, it writes the body to its store, and answers 204, or 400 when the store
/// refuses a line. Its store holds the secret `plant.influx_token` (`s3cret`).
struct Plant {
    hub: hub::Hub,
    supervisor: Supervisor,
    clock: Clock,
    net: env::net::Net,
    tasks: Tasks,
    address: SocketAddr,
    got: Rc<RefCell<Vec<Got>>>,
    answers: Rc<RefCell<VecDeque<(StatusCode, &'static str)>>>,
    /// The `Authorization` header of each request.
    auth: Rc<RefCell<Vec<Option<String>>>>,
    /// The bytes that each request writes to [`TOKEN_FILE`] as it arrives, in turn,
    /// while any are left.
    tokens: Rc<RefCell<VecDeque<&'static [u8]>>>,
    paths: env::paths::Paths,
    store: Rc<RefCell<crate::sim::Store>>,
    errors: Rc<RefCell<Vec<String>>>,
    /// Each value of `influx.status.class`, as [`read_status`] gives them.
    classes: Rc<RefCell<Vec<u8>>>,
    confirmed: Rc<RefCell<Vec<u64>>>,
    lost: Rc<RefCell<Vec<u64>>>,
    oversized: Rc<RefCell<Vec<u64>>>,
}

impl Plant {
    /// A plant whose InfluxDB serves from the start.
    async fn new(node: ::sim::node::Node, tasks: Tasks) -> Self {
        let plant = Self::unserved(node, tasks).await;
        plant.serve();
        plant
    }

    /// A plant whose InfluxDB refuses each connection until [`Plant::serve`].
    async fn unserved(node: ::sim::node::Node, tasks: Tasks) -> Self {
        let address = SocketAddr::new(node.addresses()[0], PORT);
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let kinds = Table::new().with("influx", Kind);
        let inputs = testing::create_config(env, node.net(), kinds).await;
        let token = "plant.influx_token".parse().expect("a name");
        testing::create_secret(&node.files(), &token, b"s3cret").await;
        define(&inputs);
        let errors = read_errors(&inputs.hub, &tasks).await;
        let classes =
            read_status(&inputs.hub, &tasks, "class", |sample| sample[0]).await;
        let confirmed = read_confirmed(&inputs.hub, &tasks).await;
        let lost = read_count(&inputs.hub, &tasks, "lost").await;
        let oversized = read_count(&inputs.hub, &tasks, "oversized").await;
        Self {
            hub: inputs.hub.clone(),
            supervisor: Supervisor::new(inputs),
            clock: node.clock(),
            net: node.net(),
            tasks,
            address,
            got: Rc::default(),
            answers: Rc::default(),
            auth: Rc::default(),
            tokens: Rc::default(),
            paths: node.paths(),
            store: Rc::default(),
            errors,
            classes,
            confirmed,
            lost,
            oversized,
        }
    }

    /// Starts the InfluxDB of the plant.
    fn serve(&self) {
        let listen = tcp::Listen {
            local: self.address,
            backlog: 4,
            options: OPTIONS,
        };
        let listener = self.net.listen(&listen).expect("the port is free");
        let (got, next, store) = (
            Rc::clone(&self.got),
            Rc::clone(&self.answers),
            Rc::clone(&self.store),
        );
        let (auth, tokens, paths) = (
            Rc::clone(&self.auth),
            Rc::clone(&self.tokens),
            self.paths.clone(),
        );
        let answer = move |request: Request<Bytes>| {
            let body = String::from_utf8(request.body().to_vec()).expect("UTF-8");
            got.borrow_mut().push((request.uri().to_string(), body));
            let header = request.headers().get(http::header::AUTHORIZATION);
            auth.borrow_mut()
                .push(header.map(|header| header.to_str().expect("ASCII").to_owned()));
            if let Some(token) = tokens.borrow_mut().pop_front() {
                let path = std::path::Path::new(TOKEN_FILE);
                paths.write(path, token).expect("writes the token");
            }
            let next = next.borrow_mut().pop_front();
            let next = next.map(|(status, text)| (status, text.to_owned()));
            let (status, text) = next.unwrap_or_else(|| {
                match store.borrow_mut().write(request.body()) {
                    Ok(()) => (StatusCode::NO_CONTENT, String::new()),
                    Err(error) => (StatusCode::BAD_REQUEST, error.to_string()),
                }
            });
            let mut response = Response::new(Bytes::from(text));
            *response.status_mut() = status;
            response
        };
        let server = connector::http::sim::serve(listener, self.tasks.clone(), answer);
        self.tasks.spawn(async move {
            let error = server.await;
            panic!("the listener failed: {error}");
        });
    }

    /// The config of an influx connector at this plant's InfluxDB.
    fn config(&self) -> Document {
        config(string(&format!("http://{}", self.address)))
    }

    /// Supervises the connector `influx` with `config` until the token it gives is
    /// cancelled.
    fn start(&self, config: Document) -> (cancel::Token, End) {
        let (token, end) = (cancel::Token::new(), Rc::new(RefCell::new(None)));
        let (supervisor, cancel, into) =
            (self.supervisor.clone(), token.clone(), Rc::clone(&end));
        self.tasks.spawn(async move {
            let name = "influx".parse().expect("a name");
            let opened = supervisor.open("influx", name, &config, &cancel).await;
            *into.borrow_mut() = Some(opened.run().await);
        });
        (token, end)
    }

    /// Cancels the run of `token`, and gives its end.
    async fn stop(&self, (token, end): (cancel::Token, End)) -> Result<(), Error> {
        token.cancel();
        self.clock.sleep(Span::SECOND).await;
        end.borrow_mut().take().expect("the run ended")
    }

    async fn writer(&self, channels: &[&str]) -> hub::writer::Writer {
        let config = hub::writer::Config {
            subject: name("edge.writer"),
            authority: Authority::ABSOLUTE,
            lease: None,
            channels: channels.iter().map(|channel| name(channel)).collect(),
        };
        self.hub.writer(config).await.expect("opens")
    }

    fn got(&self) -> Vec<Got> {
        self.got.borrow().clone()
    }
}

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

fn define(inputs: &supervisor::Config) {
    let time = channel::Key::from_u128(1);
    let on = |index, key, data_type| {
        let data_type = DataType::Sample(data_type);
        let data = Data::new(index, None, data_type, None).expect("no unit");
        let key = channel::Key::from_u128(key);
        let kind = spec::channel::Kind::Data(data);
        Definition::Channel(Channel { key, kind })
    };
    let data = |key, scalar| on(time, key, Type::Scalar(scalar));
    let site = |key| on(channel::Key::from_u128(6), key, Type::Scalar(Scalar::F64));
    let index = spec::channel::Kind::Index {
        error: None,
        control: None,
    };
    let mut definitions = testing::create_status(
        &name("influx"),
        &[name(CONFIRMED), name(LOST), name(OVERSIZED)],
        channel::Key::from_u128(STATUS),
    );
    definitions.extend([
        (
            name("edge.time"),
            Definition::Channel(Channel {
                key: time,
                kind: index.clone(),
            }),
        ),
        (name("edge.a"), data(2, Scalar::F64)),
        (name("edge.b"), data(3, Scalar::F64)),
        (name("edge.count"), data(4, Scalar::I64)),
        (name("time"), data(5, Scalar::F64)),
        (
            name("site.time"),
            Definition::Channel(Channel {
                key: channel::Key::from_u128(6),
                kind: index,
            }),
        ),
        (name("site.c"), site(7)),
    ]);
    let typed =
        TYPED.map(|(text, key, data_type)| (name(text), on(time, key, data_type)));
    definitions.extend(typed);
    inputs
        .hub
        .set_definitions(definitions.iter().map(|(name, def)| (name, def)));
}

/// Reads each sample of the status channel `influx.status.<channel>`, in a task on
/// `tasks`, into the vector it gives, with no sample equal to the one before it.
/// `read` gives the value of the bytes of one sample.
async fn read_status<T: PartialEq + 'static>(
    hub: &hub::Hub,
    tasks: &Tasks,
    channel: &str,
    read: fn(&[u8]) -> T,
) -> Rc<RefCell<Vec<T>>> {
    let channel = name(&format!("influx.status.{channel}"));
    let open = hub::reader::Config {
        select: Selector::new([channel.as_str()]).expect("a selector"),
        mode: Mode::Complete,
        subject: name("test"),
        name: None,
        hold: Span::ZERO,
    };
    let mut reader = hub.reader(open).await.expect("the reader opens");
    let values = Rc::new(RefCell::new(Vec::new()));
    let into = Rc::clone(&values);
    tasks.spawn(async move {
        let mut raw = Vec::new();
        while let Ok(received) = reader.next().await {
            let (view, set) = (received.view(), received.set());
            for (at, bytes) in view.iter() {
                if received.name(at) != channel.as_str() {
                    continue;
                }
                let entry = &set.entries()[at];
                let range = view.range(entry.group).expect("a range");
                let count = usize::try_from(range.count).expect("fits");
                codec::decode(entry.data_type, count, bytes, &mut raw)
                    .expect("decodes");
                let samples: Vec<&[u8]> = match codec::Variable::of(entry.data_type) {
                    Some(text) => text.samples(count, &raw).collect(),
                    None => raw.chunks_exact(raw.len() / count).collect(),
                };
                let mut into = into.borrow_mut();
                for sample in samples.into_iter().map(read) {
                    if into.last() != Some(&sample) {
                        into.push(sample);
                    }
                }
            }
        }
    });
    values
}

/// Reads each text of `influx.status.error`, as [`read_status`] does.
async fn read_errors(hub: &hub::Hub, tasks: &Tasks) -> Rc<RefCell<Vec<String>>> {
    read_status(hub, tasks, "error", |sample| {
        std::str::from_utf8(sample).expect("UTF-8").to_owned()
    })
    .await
}

/// Reads each value of `influx.status.confirmed`, as [`read_status`] does.
async fn read_confirmed(hub: &hub::Hub, tasks: &Tasks) -> Rc<RefCell<Vec<u64>>> {
    read_count(hub, tasks, "confirmed").await
}

/// Reads each value of the count `influx.status.<count>`, as [`read_status`] does.
async fn read_count(
    hub: &hub::Hub,
    tasks: &Tasks,
    count: &str,
) -> Rc<RefCell<Vec<u64>>> {
    read_status(hub, tasks, count, |sample| {
        u64::from_le_bytes(sample.try_into().expect("8 bytes"))
    })
    .await
}

/// Writes one frame of `edge.a` and `edge.b` through `writer`, one sample time for
/// each of `samples`, at `first` and each nanosecond after.
fn write(writer: &mut hub::writer::Writer, first: i64, samples: &[(f64, f64)]) {
    let a: Vec<_> = samples.iter().map(|&(a, _)| a).collect();
    let b: Vec<_> = samples.iter().map(|&(_, b)| b).collect();
    write_columns(writer, first, &[(2, &a), (3, &b)]);
}

/// Writes one frame through `writer` with the samples of each channel key of
/// `columns`, which each hold the same count, at `first` and each nanosecond after.
fn write_columns(
    writer: &mut hub::writer::Writer,
    first: i64,
    columns: &[(u128, &[f64])],
) {
    let outcomes = write_draft(writer, first, columns);
    let applied = outcomes
        .iter()
        .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }));
    assert!(applied, "the frame applies: {outcomes:?}");
}

/// Writes one frame as [`write_columns`] does, and gives the outcome of each group.
fn write_draft(
    writer: &mut hub::writer::Writer,
    first: i64,
    columns: &[(u128, &[f64])],
) -> Vec<hub::home::Outcome> {
    let count = columns.first().map_or(0, |(_, samples)| samples.len());
    let bytes: Vec<(u128, Vec<u8>)> = (columns.iter())
        .map(|&(key, samples)| {
            (key, samples.iter().flat_map(|s| s.to_le_bytes()).collect())
        })
        .collect();
    let raw: Vec<(u128, &[u8])> = bytes
        .iter()
        .map(|(key, bytes)| (*key, bytes.as_slice()))
        .collect();
    write_raw(writer, first, count, &raw)
}

/// Writes one frame through `writer` of `count` sample times at `first` and each
/// nanosecond after, with the raw samples of each channel key of `columns`, and gives
/// the outcome of each group.
fn write_raw(
    writer: &mut hub::writer::Writer,
    first: i64,
    count: usize,
    columns: &[(u128, &[u8])],
) -> Vec<hub::home::Outcome> {
    let entries = writer.set().entries();
    let at = |key| {
        let key = channel::Key::from_u128(key);
        entries
            .iter()
            .position(|entry| entry.key == key)
            .expect("written")
    };
    let time = at(1);
    let group = entries[time].group;
    let mut series: Vec<_> = std::iter::once((time, count * 8))
        .chain(columns.iter().map(|&(key, bytes)| (at(key), bytes.len())))
        .collect();
    series.sort_unstable();
    let mut draft = writer.draft(Form::Raw, &series).expect("a frame");
    let times = (first..).take(count).flat_map(i64::to_le_bytes);
    fill(draft.series_mut(time).expect("the index"), times);
    for &(key, bytes) in columns {
        fill(
            draft.series_mut(at(key)).expect("a series"),
            bytes.iter().copied(),
        );
    }
    draft.set_count(group, u32::try_from(count).expect("fits"));
    let outcomes = writer.write(Label::Path(Path::Live), draft);
    outcomes.expect("the hub takes the frame").to_vec()
}

/// Writes one frame of `edge.a` and `edge.b` at `first` and the 2 nanoseconds after,
/// which is lost for no room, and gives the seqs it lost. Takes drafts from the pool
/// until it has no room, frees the last, writes, and frees the rest.
fn write_lost(writer: &mut hub::writer::Writer, first: i64) -> std::ops::Range<u64> {
    let index = (writer.set().entries().iter())
        .position(|entry| entry.key == channel::Key::from_u128(1))
        .expect("written");
    let mut drafts = Vec::new();
    let mut len: usize = 1 << 23;
    while len > 0 {
        while let Ok(draft) = writer.draft(Form::Raw, &[(index, len)]) {
            drafts.push(draft);
        }
        len -= len.div_ceil(16);
    }
    drop(drafts.pop());
    let outcomes = write_draft(writer, first, &[(2, &[1.0; 3]), (3, &[2.0; 3])]);
    let [hub::home::Outcome::Lost { range, .. }] = outcomes[..] else {
        panic!("the frame is lost for no room: {outcomes:?}");
    };
    range.seq..range.end().expect("an end")
}

/// Writes one frame of only the index `edge.time`, of `count` samples at `first` and
/// each nanosecond after.
fn write_times(writer: &mut hub::writer::Writer, first: i64, count: usize) {
    let time = (writer.set().entries().iter())
        .position(|entry| entry.key == channel::Key::from_u128(1))
        .expect("written");
    let group = writer.set().entries()[time].group;
    let mut draft = writer
        .draft(Form::Raw, &[(time, count * 8)])
        .expect("a frame");
    let times = (first..).take(count).flat_map(i64::to_le_bytes);
    fill(draft.series_mut(time).expect("the index"), times);
    draft.set_count(group, u32::try_from(count).expect("fits"));
    let outcomes = writer.write(Label::Path(Path::Live), draft);
    let outcomes = outcomes.expect("the hub takes the frame");
    let applied = outcomes
        .iter()
        .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }));
    assert!(applied, "the frame applies: {outcomes:?}");
}

fn fill(series: &mut [u8], bytes: impl Iterator<Item = u8>) {
    for (to, from) in series.iter_mut().zip(bytes) {
        *to = from;
    }
}

/// The line of `edge.a` and `edge.b` at `time`, as the kind writes it.
fn line(a: &str, b: &str, time: i64) -> String {
    format!("telemetry edge.a={a},edge.b={b} {time}\n")
}

#[test]
fn writes_one_line_for_each_sample_time_to_its_database() {
    let got = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(with(plant.config(), "database", &string("plant a/b")));
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        let samples = [(1.5, 2.0), (f64::NAN, 4.0), (f64::NAN, f64::INFINITY)];
        write(&mut writer, first, &samples);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let (got, first) = got;
    let body =
        line("1.5e0", "2e0", first) + &format!("telemetry edge.b=4e0 {}\n", first + 1);
    assert_eq!(
        got,
        vec![("/write?db=plant%20a%2Fb&precision=ns".into(), body)]
    );
}

const WRITE: &str = "/write?db=plant&precision=ns";

#[test]
fn writes_to_a_bucket_in_its_org() {
    let got = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let target = [(60, "bucket", string("edge/1")), (62, "org", string("a b"))];
        let run = plant.start(plus(without(plant.config(), "database"), &target));
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let (got, first) = got;
    let uri = "/api/v2/write?org=a%20b&bucket=edge%2F1&precision=ns";
    assert_eq!(got, vec![(uri.into(), line("1e0", "2e0", first))]);
}

/// Each byte of `/` takes three bytes in the query, the most that one byte can take.
#[test]
fn writes_to_a_bucket_and_an_org_of_the_longest_names() {
    let longest = "/".repeat(4_096);
    let (got, first) = run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let target = [
            (60, "bucket", string(&longest)),
            (62, "org", string(&longest)),
        ];
        let run = plant.start(plus(without(plant.config(), "database"), &target));
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let encoded = "%2F".repeat(4_096);
    let uri = format!("/api/v2/write?org={encoded}&bucket={encoded}&precision=ns");
    assert_eq!(got, vec![(uri, line("1e0", "2e0", first))]);
}

/// The fix in the text of a 400 or a 422.
const REFUSED: &str = "For a field type conflict, write to another `measurement` or \
                       delete the old field in InfluxDB. For points older than the \
                       retention period, raise it or write to another bucket";
/// The fix in the text of a 404.
const UNKNOWN: &str =
    "Create the database, the bucket, or the org in InfluxDB, or name one that exists";

/// An answer of InfluxDB 1 to a batch of bad lines, of 65537 bytes or more.
fn large() -> String {
    let line = "unable to parse 'telemetry a=': missing field value\n";
    line.repeat((1 << 16) / line.len() + 1)
}

/// The status text of the ASCII error `full`, over 1024 bytes.
fn shown(full: &str) -> String {
    let head: String = full.chars().take(510).collect();
    let tail: String = full.chars().skip(full.len() - 510).collect();
    format!("{head}…{tail}")
}

#[test]
fn sends_a_batch_again_after_each_other_answer_and_shows_the_error_until_a_204() {
    let (got, errors, confirmed, points) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let large: &'static str = large().leak();
        plant.answers.borrow_mut().extend([
            (StatusCode::INTERNAL_SERVER_ERROR, " the disk is full \n"),
            (StatusCode::SERVICE_UNAVAILABLE, "starting"),
            (StatusCode::UNAUTHORIZED, "unauthorized access"),
            (StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
            (StatusCode::TOO_MANY_REQUESTS, "slow down"),
            (StatusCode::BAD_REQUEST, "field type conflict"),
            (StatusCode::UNPROCESSABLE_ENTITY, "beyond retention policy"),
            (StatusCode::NOT_FOUND, "database not found: \"plant\""),
            (StatusCode::BAD_REQUEST, large),
            (StatusCode::NOT_FOUND, large),
            (StatusCode::OK, "ok"),
        ]);
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(600_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let got = (plant.got().into_iter())
            .map(|(uri, body)| (uri, body == line("1e0", "2e0", first)))
            .collect::<Vec<_>>();
        let points = plant.store.borrow().points("telemetry", &[]).count();
        let confirmed = plant.confirmed.borrow().clone();
        (got, plant.errors.borrow().clone(), confirmed, points)
    });
    let head = large().chars().take(1 << 16).collect::<String>();
    let head = head.trim();
    assert_eq!(got, vec![(WRITE.to_owned(), true); 12]);
    assert_eq!(
        errors,
        [
            String::new(),
            "InfluxDB answered 500 Internal Server Error: the disk is full".into(),
            "InfluxDB answered 503 Service Unavailable: starting".into(),
            "InfluxDB answered 401 Unauthorized: unauthorized access".into(),
            "InfluxDB answered 413 Payload Too Large: request too large".into(),
            "InfluxDB answered 429 Too Many Requests: slow down".into(),
            format!(
                "InfluxDB answered 400 Bad Request: field type conflict. {REFUSED}"
            ),
            format!(
                "InfluxDB answered 422 Unprocessable Entity: beyond retention policy. \
                 {REFUSED}"
            ),
            format!(
                "InfluxDB answered 404 Not Found: database not found: \"plant\". \
                 {UNKNOWN}"
            ),
            shown(&format!(
                "InfluxDB answered 400 Bad Request: {head}. {REFUSED}"
            )),
            shown(&format!(
                "InfluxDB answered 404 Not Found: {head}. {UNKNOWN}"
            )),
            "InfluxDB answered 200 OK: ok".into(),
            String::new(),
        ]
    );
    assert_eq!((confirmed, points), (vec![0, 2], 1));
}

/// The config of `plant` with `token` set to a call of `function` with `argument`.
fn with_token(plant: &Plant, function: &str, argument: &str) -> Document {
    plus(plant.config(), &[(60, "token", call(function, argument))])
}

/// Runs `config` on `plant` with one sample written, until 20 s after it, and gives
/// the `Authorization` header of each request, the errors, and the end of the run.
async fn send_one(
    plant: &Plant,
    config: Document,
) -> (Vec<Option<String>>, Vec<String>, Option<Result<(), Error>>) {
    let run = plant.start(config);
    let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
    plant.clock.sleep(Span::SECOND).await;
    let first = writer.now().nanos();
    write(&mut writer, first, &[(1.0, 2.0)]);
    plant.clock.sleep(Span::from_nanos(20_000_000_000)).await;
    let end = run.1.borrow_mut().take();
    run.0.cancel();
    plant.clock.sleep(Span::SECOND).await;
    let auth = plant.auth.borrow().clone();
    (auth, plant.errors.borrow().clone(), end)
}

#[test]
fn sends_the_token_of_a_store_or_a_file_in_each_request() {
    let got = [("secret", "plant.influx_token"), ("file", TOKEN_FILE)].map(
        |(function, argument)| {
            run(move |node, tasks| async move {
                node.paths()
                    .write(std::path::Path::new(TOKEN_FILE), b"f1le\n")
                    .expect("writes");
                let plant = Plant::new(node, tasks).await;
                let config = with_token(&plant, function, argument);
                let (auth, errors, end) = send_one(&plant, config).await;
                let points = plant.store.borrow().points("telemetry", &[]).count();
                (auth, errors, end.is_none(), points)
            })
        },
    );
    assert_eq!(
        got,
        [
            (
                vec![Some("Token s3cret".to_owned())],
                vec![String::new()],
                true,
                1
            ),
            (
                vec![Some("Token f1le".to_owned())],
                vec![String::new()],
                true,
                1
            ),
        ]
    );
}

#[test]
fn reads_the_token_again_after_a_401_or_a_403_and_writes_each_sample_once() {
    for (status, text) in [
        (StatusCode::UNAUTHORIZED, "401 Unauthorized"),
        (StatusCode::FORBIDDEN, "403 Forbidden"),
    ] {
        let got = run(move |node, tasks| async move {
            let plant = Plant::new(node.clone(), tasks).await;
            node.paths()
                .write(std::path::Path::new(TOKEN_FILE), b"old")
                .expect("writes");
            let body = r#"{"code":"unauthorized","message":"unauthorized access"}"#;
            plant.answers.borrow_mut().push_back((status, body));
            plant.tokens.borrow_mut().push_back(b"new\n");
            let config = with_token(&plant, "file", TOKEN_FILE);
            let (auth, errors, end) = send_one(&plant, config).await;
            (
                auth,
                errors,
                end.is_none(),
                plant.store.borrow().points("telemetry", &[]).count(),
            )
        });
        assert_eq!(
            got,
            (
                vec![Some("Token old".to_owned()), Some("Token new".to_owned())],
                vec![
                    String::new(),
                    format!(
                        "InfluxDB answered {text}. Check the token \
                         file(\"{TOKEN_FILE}\"): {{\"code\":\"unauthorized\",\
                         \"message\":\"unauthorized access\"}}"
                    ),
                    String::new(),
                ],
                true,
                1,
            ),
            "{status}"
        );
    }
}

#[test]
fn keeps_the_token_after_an_answer_other_than_401_or_403() {
    let got = run(|node, tasks| async move {
        let plant = Plant::new(node.clone(), tasks).await;
        node.paths()
            .write(std::path::Path::new(TOKEN_FILE), b"old")
            .expect("writes");
        (plant.answers.borrow_mut())
            .push_back((StatusCode::INTERNAL_SERVER_ERROR, "the disk is full"));
        plant.tokens.borrow_mut().push_back(b"new");
        let config = with_token(&plant, "file", TOKEN_FILE);
        let (auth, errors, end) = send_one(&plant, config).await;
        let points = plant.store.borrow().points("telemetry", &[]).count();
        (auth, errors, end.is_none(), points)
    });
    assert_eq!(
        got,
        (
            vec![Some("Token old".to_owned()); 2],
            vec![
                String::new(),
                "InfluxDB answered 500 Internal Server Error: the disk is full"
                    .to_owned(),
                String::new(),
            ],
            true,
            1,
        )
    );
}

#[test]
fn reads_the_token_again_after_a_401_whose_body_is_over_the_cap() {
    let got = run(move |node, tasks| async move {
        let plant = Plant::new(node.clone(), tasks).await;
        (node.paths())
            .write(std::path::Path::new(TOKEN_FILE), b"old")
            .expect("writes");
        let body: &'static str = large().leak();
        (plant.answers.borrow_mut()).push_back((StatusCode::UNAUTHORIZED, body));
        plant.tokens.borrow_mut().push_back(b"new\n");
        let config = with_token(&plant, "file", TOKEN_FILE);
        let (auth, errors, end) = send_one(&plant, config).await;
        (auth, errors, end.is_none())
    });
    let head = large().chars().take(1 << 16).collect::<String>();
    let full = format!(
        "InfluxDB answered 401 Unauthorized. Check the token file({TOKEN_FILE:?}): {}",
        head.trim()
    );
    assert_eq!(
        got,
        (
            vec![Some("Token old".to_owned()), Some("Token new".to_owned())],
            vec![String::new(), shown(&full), String::new()],
            true,
        )
    );
}

#[test]
fn keeps_the_source_of_the_token_after_a_long_401_body() {
    let got = run(|node, tasks| async move {
        let plant = Plant::new(node.clone(), tasks).await;
        node.paths()
            .write(std::path::Path::new(TOKEN_FILE), b"old")
            .expect("writes");
        let body: &'static str = "x".repeat(1100).leak();
        plant
            .answers
            .borrow_mut()
            .push_back((StatusCode::UNAUTHORIZED, body));
        plant.tokens.borrow_mut().push_back(b"new");
        let config = with_token(&plant, "file", TOKEN_FILE);
        let (_, errors, _) = send_one(&plant, config).await;
        errors
    });
    let head = format!(
        "InfluxDB answered 401 Unauthorized. Check the token file(\"{TOKEN_FILE}\"): "
    );
    let shown = format!("{head}{}…{}", "x".repeat(510 - head.len()), "x".repeat(510));
    assert_eq!(got, [String::new(), shown, String::new()]);
}

#[test]
fn keeps_the_answer_of_influx_after_a_401_with_a_long_source_and_body() {
    let path: &'static str = format!("/run/secrets/{}", "\u{1}".repeat(200)).leak();
    let body: &'static str = "y".repeat(600).leak();
    let got = run(move |node, tasks| async move {
        let plant = Plant::new(node.clone(), tasks).await;
        node.paths()
            .write(std::path::Path::new(path), b"old")
            .expect("writes");
        (plant.answers.borrow_mut()).push_back((StatusCode::UNAUTHORIZED, body));
        let config = with_token(&plant, "file", path);
        let (auth, errors, _) = send_one(&plant, config).await;
        (auth, errors)
    });
    let full = format!(
        "InfluxDB answered 401 Unauthorized. Check the token file({path:?}): {body}"
    );
    let head: String = full.chars().take(510).collect();
    let tail: String = full.chars().skip(full.len() - 510).collect();
    let want = format!("{head}…{tail}");
    assert!(want.starts_with(
        "InfluxDB answered 401 Unauthorized. Check the token file(\"/run/secrets/"
    ));
    assert!(want.ends_with(&"y".repeat(510)));
    assert_eq!(
        got,
        (
            vec![Some("Token old".to_owned()); 2],
            vec![String::new(), want, String::new()]
        )
    );
}

#[test]
fn ends_the_run_when_the_token_is_gone_after_a_401() {
    let got = run(|node, tasks| async move {
        let plant = Plant::new(node.clone(), tasks).await;
        node.paths()
            .write(std::path::Path::new(TOKEN_FILE), b"old")
            .expect("writes");
        plant
            .answers
            .borrow_mut()
            .push_back((StatusCode::UNAUTHORIZED, "no"));
        plant.tokens.borrow_mut().push_back(b"");
        let config = with_token(&plant, "file", TOKEN_FILE);
        let (auth, errors, _) = send_one(&plant, config).await;
        let classes = plant.classes.borrow().clone();
        (auth, errors, classes)
    });
    assert_eq!(
        got,
        (
            vec![Some("Token old".to_owned())],
            vec![
                String::new(),
                format!(
                    "InfluxDB answered 401 Unauthorized. Check the token \
                     file(\"{TOKEN_FILE}\"): no"
                ),
                format!(
                    "the secret file(\"{TOKEN_FILE}\"): the file {TOKEN_FILE} is \
                     empty. Write the value to it"
                ),
            ],
            vec![0, RETRY],
        )
    );
}

#[test]
fn ends_the_run_at_a_token_that_a_header_cannot_hold() {
    let got = run(|node, tasks| async move {
        node.paths()
            .write(std::path::Path::new(TOKEN_FILE), b"t0\nken\n")
            .expect("writes");
        let plant = Plant::new(node, tasks).await;
        let config = with_token(&plant, "file", TOKEN_FILE);
        let (auth, errors, _) = send_one(&plant, config).await;
        let classes = plant.classes.borrow().clone();
        (auth, errors, classes)
    });
    assert_eq!(
        got,
        (
            vec![],
            vec![
                String::new(),
                format!(
                    "the token file(\"{TOKEN_FILE}\") holds a byte that an HTTP header \
                     cannot hold. Write the token with no control byte"
                ),
            ],
            vec![0, RETRY],
        )
    );
}

/// The last error of a run with a token in a file at a path of 200 control bytes,
/// which holds `value`.
fn last_error_of_a_long_source(value: &'static [u8]) -> String {
    let path: &'static str = format!("/run/secrets/{}", "\u{1}".repeat(200)).leak();
    let errors = run(move |node, tasks| async move {
        node.paths()
            .write(std::path::Path::new(path), value)
            .expect("writes");
        let plant = Plant::new(node, tasks).await;
        let config = with_token(&plant, "file", path);
        send_one(&plant, config).await.1
    });
    errors.last().cloned().expect("an error")
}

#[test]
fn keeps_the_reason_of_a_bad_token_with_a_long_source() {
    let got = last_error_of_a_long_source(b"t0\nken\n");
    assert!(got.starts_with("the token file(\"/run/secrets/"), "{got}");
    assert!(
        got.ends_with("cannot hold. Write the token with no control byte"),
        "{got}"
    );
}

#[test]
fn keeps_the_reason_of_an_empty_token_file_with_a_long_source() {
    let got = last_error_of_a_long_source(b"");
    assert!(got.starts_with("the secret file(\"/run/secrets/"), "{got}");
    assert!(got.ends_with("is empty. Write the value to it"), "{got}");
}

#[test]
fn confirms_the_samples_of_each_batch_after_its_204_and_no_nan_or_infinity() {
    let confirmed = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        let samples = [(1.0, 2.0), (f64::NAN, 3.0), (f64::NAN, f64::INFINITY)];
        write(&mut writer, first, &samples);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        write(&mut writer, first + 3, &[(4.0, 5.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        plant.confirmed.borrow().clone()
    });
    assert_eq!(confirmed, [0, 3, 5]);
}

#[test]
fn keeps_the_confirmed_count_across_a_restart_of_the_run() {
    let confirmed = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        let open = hub::reader::Config {
            select: Selector::new(["edge.a"]).expect("a selector"),
            mode: Mode::Complete,
            subject: name("influx"),
            name: Some(name("influx")),
            hold: Span::ZERO,
        };
        let replaced = plant.hub.reader(open).await.expect("the reader opens");
        plant.clock.sleep(Span::from_nanos(5_000_000_000)).await;
        drop(replaced);
        write(&mut writer, first + 1, &[(3.0, 4.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        plant.confirmed.borrow().clone()
    });
    assert_eq!(confirmed, [0, 2, 4]);
}

#[test]
fn writes_the_gap_line_of_a_run_lost_for_no_room_and_counts_its_seqs_after_a_204() {
    let (lost, stamp, bodies, counts) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        drop(plant.writer(&["site.time", "site.c"]).await);
        let channels = ["site.time", "site.c", "edge.time", "edge.a", "edge.b"];
        let mut writer = plant.writer(&channels).await;
        let index = (writer.set().entries().iter())
            .position(|entry| entry.key == channel::Key::from_u128(1));
        assert_eq!(index, Some(2), "the group of `site.time` comes first");
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        let sent = plant.got().len();
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
        plant.answers.borrow_mut().push_back(failed);
        let lost = write_lost(&mut writer, first + 1);
        write(&mut writer, first + 4, &[(3.0, 4.0)]);
        plant.clock.sleep(Span::from_nanos(5_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let bodies: Vec<_> = (plant.got().into_iter().skip(sent))
            .map(|(_, body)| body)
            .collect();
        let counts = (
            plant.lost.borrow().clone(),
            plant.confirmed.borrow().clone(),
        );
        (lost, first + 4, bodies, counts)
    });
    assert_eq!(lost.end - lost.start, 3);
    let body = format!(
        "foundation_gaps,connector=influx,index=edge.time,path=live count=3i \
         {stamp}\n{}",
        line("3e0", "4e0", stamp),
    );
    assert_eq!(bodies, [body.clone(), body]);
    assert_eq!(counts, (vec![0, 3], vec![0, 2, 4]));
}

/// What InfluxDB gets after a run lost for no room, then `after`: the body of the
/// first run, the count `lost`, and the body of a second run, which gets one more
/// frame at the stamp after `after`. The stamp is that of the first sample after the
/// loss.
fn after_a_loss(
    after: fn(&mut hub::writer::Writer, i64),
) -> (String, Vec<u64>, String, i64) {
    run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        let run = plant.start(plant.config());
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        let sent = plant.got().len();
        write_lost(&mut writer, first + 1);
        after(&mut writer, first + 4);
        plant.clock.sleep(Span::from_nanos(5_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let bodies = |got: Vec<Got>, skip| -> String {
            got.into_iter().skip(skip).map(|(_, body)| body).collect()
        };
        let body = bodies(std::mem::take(&mut *plant.got.borrow_mut()), sent);
        let lost = plant.lost.borrow().clone();
        let run = plant.start(plant.config());
        plant.clock.sleep(Span::SECOND).await;
        write(&mut writer, first + 10, &[(5.0, 6.0)]);
        plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        (body, lost, bodies(plant.got(), 0), first + 4)
    })
}

/// The gap line sits at the first sample time after the gap, also when the run leaves
/// its line out, and counts only the seqs of the gap. The batch acks the frame, so the
/// next run gets no gap again.
#[test]
fn writes_the_gap_line_at_a_first_sample_time_whose_line_is_left_out() {
    let (body, lost, again, stamp) = after_a_loss(|writer, at| {
        write(writer, at, &[(f64::NAN, f64::INFINITY), (3.0, 4.0)]);
    });
    let gap = "foundation_gaps,connector=influx,index=edge.time,path=live";
    let data = line("3e0", "4e0", stamp + 1);
    assert_eq!(body, format!("{gap} count=3i {stamp}\n{data}"));
    assert_eq!(lost, [0, 3]);
    assert_eq!(again, line("5e0", "6e0", stamp + 6));
}

#[test]
fn writes_the_gap_line_at_the_first_stamp_of_a_frame_with_no_data_channel() {
    let (body, lost, again, stamp) = after_a_loss(|writer, at| {
        write_times(writer, at, 2);
        write(writer, at + 2, &[(3.0, 4.0)]);
    });
    let gap = "foundation_gaps,connector=influx,index=edge.time,path=live";
    let data = line("3e0", "4e0", stamp + 2);
    assert_eq!(body, format!("{gap} count=3i {stamp}\n{data}"));
    assert_eq!(lost, [0, 3]);
    assert_eq!(again, line("5e0", "6e0", stamp + 6));
}

#[test]
fn confirms_no_sample_of_a_batch_until_its_204() {
    let (failing, confirmed) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
        plant.answers.borrow_mut().extend([failed; 100]);
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(20_000_000_000)).await;
        let failing = (plant.confirmed.borrow().clone(), plant.got().len());
        plant.answers.borrow_mut().clear();
        plant.clock.sleep(Span::from_nanos(70_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        (failing, plant.confirmed.borrow().clone())
    });
    let (values, requests) = failing;
    assert_eq!(values, [0]);
    assert!(
        requests > 1,
        "InfluxDB got {requests} requests while it failed"
    );
    assert_eq!(confirmed, [0, 2]);
}

/// The bodies that the InfluxDB of a plant gets in each run, one run after the other,
/// and the errors that the last run shows. InfluxDB answers each request of the first
/// run with 500, or with 204 when `accepted`, and each later one with 204. Each run
/// writes one frame of one sample time, at which each channel holds its value of
/// `values`.
fn runs<const N: usize>(
    accepted: bool,
    values: [f64; N],
) -> ([Vec<String>; N], Vec<String>) {
    run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        if !accepted {
            let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
            plant.answers.borrow_mut().extend([failed; 100]);
        }
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        let mut bodies = Vec::new();
        for value in values {
            plant.errors.borrow_mut().clear();
            let run = plant.start(plant.config());
            plant.clock.sleep(Span::SECOND).await;
            let first = writer.now().nanos();
            write(&mut writer, first, &[(value, value)]);
            plant.clock.sleep(Span::from_nanos(3_000_000_000)).await;
            plant.stop(run).await.expect("ok after a cancel");
            plant.answers.borrow_mut().clear();
            let got = std::mem::take(&mut *plant.got.borrow_mut());
            bodies.push(got.into_iter().map(|(_, body)| body).collect::<Vec<_>>());
        }
        let errors = plant.errors.borrow().clone();
        (bodies.try_into().expect("a body list for each run"), errors)
    })
}

#[test]
fn acks_no_batch_before_a_204() {
    let ([first, second, third], errors) = runs(false, [1.0, 2.0, 3.0]);
    assert!(first.len() > 1, "{first:?}");
    let sent = first.first().expect("a request");
    assert!(first.iter().all(|body| body == sent), "{first:?}");
    assert!(
        sent.starts_with("telemetry edge.a=1e0,edge.b=1e0 "),
        "{sent}"
    );
    assert_eq!(second.len(), 2, "{second:?}");
    assert_eq!(&second[0], sent);
    assert!(second[1].starts_with("telemetry edge.a=2e0,edge.b=2e0 "));
    assert_eq!(third.len(), 1, "{third:?}");
    assert!(third[0].starts_with("telemetry edge.a=3e0,edge.b=3e0 "));
    assert_eq!(errors, [""]);
}

#[test]
fn acks_a_batch_after_its_204() {
    let ([first, second], errors) = runs(true, [1.0, 2.0]);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("telemetry edge.a=1e0,edge.b=1e0 "));
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].starts_with("telemetry edge.a=2e0,edge.b=2e0 "));
    assert_eq!(errors, [""]);
}

#[test]
fn acks_a_batch_of_no_line_with_no_request() {
    let ([first, second], errors) = runs(true, [f64::NAN, 2.0]);
    assert_eq!(first, Vec::<String>::new());
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].starts_with("telemetry edge.a=2e0,edge.b=2e0 "));
    assert_eq!(errors, [""]);
}

/// The count of lines in each body that InfluxDB gets, when it answers the first
/// request with 500 and a frame of one sample time goes before frames of `times`
/// sample times each, written while that request is in flight.
fn lines_of_each_request(times: &[usize]) -> Vec<usize> {
    let times: Vec<_> = times.iter().map(|&count| (count, 1.0)).collect();
    lines_of_each_request_of(&times)
}

/// As [`lines_of_each_request`], with each `(count, value)` of `times` a frame of
/// `count` sample times at which each channel holds `value`.
fn lines_of_each_request_of(times: &[(usize, f64)]) -> Vec<usize> {
    lines_of_each_request_after(false, times)
}

/// As [`lines_of_each_request_of`], with a frame lost for no room before the frames
/// of `times` when `lost`.
fn lines_of_each_request_after(lost: bool, times: &[(usize, f64)]) -> Vec<usize> {
    let times = times.to_vec();
    run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
        plant.answers.borrow_mut().push_back(failed);
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let mut first = writer.now().nanos();
        write(&mut writer, first, &[(0.0, 0.0)]);
        plant.clock.sleep(Span::from_nanos(100_000_000)).await;
        assert_eq!(
            plant.got.borrow().len(),
            1,
            "the first request is in flight"
        );
        if lost {
            first += 10_000;
            write_lost(&mut writer, first);
        }
        for (count, value) in times {
            first += 10_000;
            write(&mut writer, first, &vec![(value, value); count]);
        }
        plant.clock.sleep(Span::from_nanos(5_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let got = plant.got();
        (got.iter()).map(|(_, body)| body.lines().count()).collect()
    })
}

#[test]
fn reads_each_frame_into_the_next_batch_while_a_request_is_in_flight() {
    assert_eq!(lines_of_each_request(&[1, 2, 3]), [1, 1, 6]);
}

#[test]
fn reads_no_frame_into_a_batch_of_5000_lines_or_more() {
    assert_eq!(
        lines_of_each_request(&[2_500, 2_499, 1, 3_000, 2_000]),
        [1, 1, 5_000, 5_000]
    );
}

/// A gap line counts toward the 5,000 lines of a full batch.
#[test]
fn reads_no_frame_into_a_batch_of_4999_lines_and_a_gap_line() {
    assert_eq!(
        lines_of_each_request_after(true, &[(2_499, 1.0), (2_500, 1.0), (1, 1.0)]),
        [1, 1, 5_000, 1]
    );
}

/// A sample time at which each value is NaN gives no line, so it does not count
/// toward the 5,000 lines of a full batch.
#[test]
fn counts_no_line_toward_a_full_batch_for_a_sample_time_of_only_nan() {
    assert_eq!(
        lines_of_each_request_of(&[
            (2_500, f64::NAN),
            (2_500, f64::NAN),
            (1, 1.0),
            (1, 1.0)
        ]),
        [1, 1, 2]
    );
}

/// A batch takes no frame once its lines hold 4 MiB, so it stays under the body limit
/// of InfluxDB 3.
#[test]
fn reads_no_frame_into_a_batch_of_4_mib_or_more() {
    let lines = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
        plant.answers.borrow_mut().push_back(failed);
        let run = plant.start(with(plant.config(), "select", &string("edge.*")));
        let mut writer = plant.writer(&["edge.time", "edge.text"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let text = "x".repeat(60_000);
        let column = variable(Type::String, &[text.as_bytes()]);
        let first = writer.now().nanos();
        for n in 0..=100 {
            let outcomes =
                write_raw(&mut writer, first + n * 10_000, 1, &[(23, &column)]);
            let applied = (outcomes.iter())
                .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }));
            assert!(applied, "the frame applies: {outcomes:?}");
            if n == 0 {
                plant.clock.sleep(Span::from_nanos(100_000_000)).await;
                let sent = plant.got.borrow().len();
                assert_eq!(sent, 1, "the first request is in flight");
            }
        }
        plant.clock.sleep(Span::from_nanos(5_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let got = plant.got();
        (got.iter())
            .map(|(_, body)| body.lines().count())
            .collect::<Vec<_>>()
    });
    assert_eq!(lines, [1, 1, 70, 30]);
}

/// What one frame gave: the end of the run once it returned, each request, the stamp
/// of the frame's first sample, and the fields of each point of the store.
struct Frame {
    end: Option<Result<(), Error>>,
    got: Vec<Got>,
    first: i64,
    points: Vec<BTreeMap<String, crate::sim::Field>>,
    /// Each value of `influx.status.oversized`.
    oversized: Vec<u64>,
}

/// Writes one frame of `count` sample times, with the raw samples of each channel of
/// `columns` (its name, key, and bytes), to a connector that selects `select`. It
/// writes to the database `plant`, or to the bucket `edge` when `bucket`, and then the
/// store takes a `u` integer, as InfluxDB 2 and 3 do.
fn one_frame(
    select: &'static str,
    bucket: bool,
    count: usize,
    columns: Vec<(&'static str, u128, Vec<u8>)>,
) -> Frame {
    run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let mut config = with(plant.config(), "select", &string(select));
        if bucket {
            *plant.store.borrow_mut() = crate::sim::Store::unsigned();
            let target = [(60, "bucket", string("edge")), (62, "org", string("o"))];
            config = plus(without(config, "database"), &target);
        }
        let (_, end) = plant.start(config);
        let names = columns.iter().map(|&(name, _, _)| name);
        let names: Vec<_> = std::iter::once("edge.time").chain(names).collect();
        let mut writer = plant.writer(&names).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        let raw: Vec<_> = (columns.iter())
            .map(|(_, key, bytes)| (*key, bytes.as_slice()))
            .collect();
        let outcomes = write_raw(&mut writer, first, count, &raw);
        let applied = outcomes
            .iter()
            .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }));
        assert!(applied, "the frame applies: {outcomes:?}");
        plant.clock.sleep(Span::SECOND).await;
        let end = end.borrow_mut().take();
        let points = (plant.store.borrow().points("telemetry", &[]))
            .map(|point| {
                let fields = point.fields.iter();
                fields.map(|(key, field)| (key.to_owned(), field)).collect()
            })
            .collect();
        Frame {
            end,
            got: plant.got(),
            first,
            points,
            oversized: plant.oversized.borrow().clone(),
        }
    })
}

/// The little-endian bytes of each sample of `samples`.
macro_rules! bytes {
    ($($sample:expr),* $(,)?) => {
        [$($sample.to_le_bytes().to_vec()),*].concat()
    };
}

#[test]
fn writes_each_scalar_type_other_than_u64_and_uuid_as_its_field() {
    let columns = vec![
        ("edge.bool", 10, vec![1, 0]),
        ("edge.i8", 11, bytes![i8::MIN, i8::MAX]),
        ("edge.i16", 12, bytes![i16::MIN, i16::MAX]),
        ("edge.i32", 13, bytes![i32::MIN, i32::MAX]),
        ("edge.count", 4, bytes![i64::MIN, i64::MAX]),
        ("edge.u8", 14, bytes![0_u8, u8::MAX]),
        ("edge.u16", 15, bytes![0_u16, u16::MAX]),
        ("edge.u32", 16, bytes![0_u32, u32::MAX]),
        ("edge.f32", 18, bytes![0.1_f32, f32::NAN]),
        (
            "edge.stamp",
            19,
            bytes![-1_i64, 1_700_000_000_123_456_789_i64],
        ),
        ("edge.span", 20, bytes![-5_i64, i64::MAX]),
    ];
    let frame = one_frame("edge.*", false, 2, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let first = frame.first;
    let body = format!(
        "telemetry edge.bool=t,edge.i8=-128i,edge.i16=-32768i,edge.i32=-2147483648i,\
         edge.count=-9223372036854775808i,edge.u8=0i,edge.u16=0i,edge.u32=0i,\
         edge.f32=1.0000000149011612e-1,edge.stamp=-1i,edge.span=-5i {first}\n\
         telemetry edge.bool=f,edge.i8=127i,edge.i16=32767i,edge.i32=2147483647i,\
         edge.count=9223372036854775807i,edge.u8=255i,edge.u16=65535i,\
         edge.u32=4294967295i,edge.stamp=1700000000123456789i,\
         edge.span=9223372036854775807i {}\n",
        first + 1
    );
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
    assert_eq!(frame.points.len(), 2);
}

#[test]
fn writes_an_f32_that_is_infinite_as_no_field() {
    let columns = vec![(
        "edge.f32",
        18,
        bytes![f32::INFINITY, f32::NEG_INFINITY, -2.5_f32],
    )];
    let frame = one_frame("edge.*", false, 3, columns);
    let body = format!("telemetry edge.f32=-2.5e0 {}\n", frame.first + 2);
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
}

#[test]
fn writes_a_u64_to_a_bucket_as_an_unsigned_integer() {
    let columns = vec![("edge.u64", 17, bytes![0_u64, u64::MAX])];
    let frame = one_frame("edge.*", true, 2, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let first = frame.first;
    let body = format!(
        "telemetry edge.u64=0u {first}\n\
         telemetry edge.u64=18446744073709551615u {}\n",
        first + 1
    );
    let uri = "/api/v2/write?org=o&bucket=edge&precision=ns";
    assert_eq!(frame.got, vec![(uri.into(), body)]);
    let field = |value| BTreeMap::from([("edge.u64".into(), value)]);
    assert_eq!(
        frame.points,
        [
            field(crate::sim::Field::Unsigned(0)),
            field(crate::sim::Field::Unsigned(u64::MAX)),
        ]
    );
}

#[test]
fn stops_with_a_config_error_at_a_u64_channel_to_a_database() {
    let frame = one_frame("edge.*", false, 1, vec![("edge.u64", 17, bytes![1_u64])]);
    assert_eq!(frame.got, []);
    assert!(frame.points.is_empty());
    let Some(Err(Error::Config(diagnostics))) = frame.end else {
        panic!("a config error, not {:?}", frame.end);
    };
    assert_eq!(
        diagnostics,
        [Diagnostic::new(
            Code::new("influx.unsigned"),
            None,
            "the channel \"edge.u64\" holds u64 samples, which InfluxDB 1 does not \
             store"
                .into(),
            "Write to a `bucket` of InfluxDB 2 or 3, or leave the channel out of \
             `select`"
                .into(),
        )]
    );
}

/// The raw series of `samples` of the variable `data_type`.
fn variable(data_type: Type, samples: &[&[u8]]) -> Vec<u8> {
    let form = codec::Variable::of(data_type).expect("a variable type");
    let mut bytes = vec![0; form.len(samples).expect("fits")];
    form.write(samples, &mut bytes);
    bytes
}

#[test]
fn writes_a_string_with_each_quote_and_backslash_escaped_and_each_newline_kept() {
    let samples: [&[u8]; 3] = [b"a\"b\\c", b"two\nlines", b""];
    let columns = vec![("edge.text", 23, variable(Type::String, &samples))];
    let frame = one_frame("edge.*", false, 3, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let first = frame.first;
    let body = format!(
        "telemetry edge.text=\"a\\\"b\\\\c\" {first}\n\
         telemetry edge.text=\"two\nlines\" {}\n\
         telemetry edge.text=\"\" {}\n",
        first + 1,
        first + 2
    );
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
    let field = |text: &str| {
        BTreeMap::from([("edge.text".into(), crate::sim::Field::String(text.into()))])
    };
    assert_eq!(
        frame.points,
        [field("a\"b\\c"), field("two\nlines"), field("")]
    );
}

#[test]
fn leaves_out_and_counts_a_string_over_65536_bytes_after_its_escapes() {
    let kept = ["é".repeat(32_768), format!("\"{}", "x".repeat(65_534))];
    let long = ["x".repeat(65_537), format!("\"{}", "x".repeat(65_535))];
    let samples = [&kept[0], &long[0], &kept[1], &long[1]].map(String::as_bytes);
    let columns = vec![
        ("edge.text", 23, variable(Type::String, &samples)),
        ("edge.count", 4, bytes![1_i64, 2_i64, 3_i64, 4_i64]),
    ];
    let frame = one_frame("edge.*", false, 4, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let first = frame.first;
    let body = format!(
        "telemetry edge.text=\"{}\",edge.count=1i {first}\n\
         telemetry edge.count=2i {}\n\
         telemetry edge.text=\"\\{}\",edge.count=3i {}\n\
         telemetry edge.count=4i {}\n",
        kept[0],
        first + 1,
        kept[1],
        first + 2,
        first + 3
    );
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
    assert_eq!(frame.points.len(), 4);
    assert_eq!(frame.oversized, [0, 2]);
}

#[test]
fn writes_a_uuid_as_its_lowercase_text() {
    let uuid = 0x0123_4567_89ab_cdef_0123_4567_89ab_cdef_u128.to_be_bytes();
    let columns = vec![("edge.uuid", 21, uuid.to_vec())];
    let frame = one_frame("edge.*", false, 1, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let body = format!(
        "telemetry edge.uuid=\"01234567-89ab-cdef-0123-456789abcdef\" {}\n",
        frame.first
    );
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
}

#[test]
fn writes_bytes_as_base64_with_padding_and_leaves_out_and_counts_over_49152() {
    let (kept, long) = (vec![0xfb_u8; 49_152], vec![0_u8; 49_153]);
    let samples: [&[u8]; 4] = [&[0, 255, 1], &[0xfb], &kept, &long];
    let columns = vec![
        ("edge.blob", 24, variable(Type::Bytes, &samples)),
        ("edge.count", 4, bytes![1_i64, 2_i64, 3_i64, 4_i64]),
    ];
    let frame = one_frame("edge.*", false, 4, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    let first = frame.first;
    let body = format!(
        "telemetry edge.blob=\"AP8B\",edge.count=1i {first}\n\
         telemetry edge.blob=\"+w==\",edge.count=2i {}\n\
         telemetry edge.blob=\"{}\",edge.count=3i {}\n\
         telemetry edge.count=4i {}\n",
        first + 1,
        "+/v7".repeat(16_384),
        first + 2,
        first + 3
    );
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
    assert_eq!(frame.points.len(), 4);
    assert_eq!(frame.oversized, [0, 1]);
}

#[test]
fn writes_empty_bytes_as_an_empty_string() {
    let columns = vec![("edge.blob", 24, variable(Type::Bytes, &[&[]]))];
    let frame = one_frame("edge.*", false, 1, columns);
    let body = format!("telemetry edge.blob=\"\" {}\n", frame.first);
    assert_eq!(frame.got, vec![(WRITE.into(), body)]);
}

#[test]
fn counts_an_oversized_string_whose_line_is_left_out() {
    let long = "x".repeat(65_537);
    let columns = vec![("edge.text", 23, variable(Type::String, &[long.as_bytes()]))];
    let frame = one_frame("edge.*", false, 1, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    assert_eq!(frame.got, []);
    assert_eq!(frame.oversized.last(), Some(&1), "{:?}", frame.oversized);
}

#[test]
fn counts_oversized_bytes_whose_line_is_left_out() {
    let long = vec![0_u8; 49_153];
    let columns = vec![("edge.blob", 24, variable(Type::Bytes, &[&long]))];
    let frame = one_frame("edge.*", false, 1, columns);
    assert!(frame.end.is_none(), "the run goes on: {:?}", frame.end);
    assert_eq!(frame.got, []);
    assert_eq!(frame.oversized.last(), Some(&1), "{:?}", frame.oversized);
}

#[test]
fn stops_with_a_config_error_at_a_channel_of_a_type_it_does_not_write() {
    let xyz = bytes![1.0_f32, 2.0_f32, 3.0_f32];
    let frame = one_frame("edge.*", true, 1, vec![("edge.xyz", 22, xyz)]);
    assert_eq!(frame.got, []);
    let Some(Err(Error::Config(diagnostics))) = frame.end else {
        panic!("a config error, not {:?}", frame.end);
    };
    assert_eq!(
        diagnostics,
        [Diagnostic::new(
            Code::new("influx.sample-type"),
            None,
            "the channel \"edge.xyz\" holds f32[3] samples, which the influx kind \
             does not write"
                .into(),
            "Leave the channel out of `select`".into(),
        )]
    );
}

#[test]
fn stops_with_a_config_error_at_a_channel_whose_name_is_no_field_key() {
    let frame = one_frame("time", false, 1, vec![("time", 5, bytes![7.0_f64])]);
    assert_eq!(frame.got, []);
    let Some(Err(Error::Config(diagnostics))) = frame.end else {
        panic!("a config error, not {:?}", frame.end);
    };
    assert_eq!(
        diagnostics,
        [Diagnostic::new(
            Code::new("influx.bad-channel"),
            None,
            "a channel name is not a field key: InfluxDB keeps the name \"time\" for \
             itself"
                .into(),
            "Rename the channel, or leave it out of `select`".into(),
        )]
    );
}

#[test]
fn writes_the_fields_of_the_channels_of_each_frame() {
    let (got, first) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        write_columns(&mut writer, first + 1, &[(3, &[3.0])]);
        write_columns(&mut writer, first + 2, &[(2, &[4.0])]);
        write_columns(&mut writer, first + 3, &[]);
        write(&mut writer, first + 4, &[(5.0, 6.0)]);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let body = [
        line("1e0", "2e0", first),
        format!("telemetry edge.b=3e0 {}\n", first + 1),
        format!("telemetry edge.a=4e0 {}\n", first + 2),
        line("5e0", "6e0", first + 4),
    ];
    let got: Vec<_> = got.into_iter().map(|(_, body)| body).collect();
    assert_eq!(got.concat(), body.concat());
}

/// The second key set holds `edge.b` at the entry where the first holds `edge.a`.
#[test]
fn names_each_field_by_the_key_set_of_its_frame() {
    let (got, first) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        drop(writer);
        let mut writer = plant.writer(&["edge.time", "edge.b"]).await;
        write_columns(&mut writer, first + 1, &[(3, &[3.0])]);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let body = [
        line("1e0", "2e0", first),
        format!("telemetry edge.b=3e0 {}\n", first + 1),
    ];
    let got: Vec<_> = got.into_iter().map(|(_, body)| body).collect();
    assert_eq!(got.concat(), body.concat());
}

#[test]
fn shows_a_write_that_fails_until_the_run_ends() {
    let errors = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let address = format!("http://{}:{}", plant.address.ip(), PORT + 1);
        let run = plant.start(with(plant.config(), "address", &string(&address)));
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(3_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        plant.errors.borrow().clone()
    });
    let refused = "the write to InfluxDB failed: the connect failed: 10.0.0.1:8087 \
                   refused the connection";
    assert_eq!(errors, ["", refused, ""]);
}

#[test]
fn holds_each_sample_once_in_time_order_after_writes_that_fail() {
    stores_each_sample_once_after_writes_that_fail(false);
}

#[test]
fn holds_each_sample_after_writes_that_fail_with_no_reader_block() {
    stores_each_sample_once_after_writes_that_fail(true);
}

/// Checks that InfluxDB stores each sample once, in time order, when it refuses each
/// connection for 6 s, then answers 500 once. The config has no `reader` block when
/// `blockless`, else that of [`config`].
fn stores_each_sample_once_after_writes_that_fail(blockless: bool) {
    let (points, accepted, errors, first) = run(move |node, tasks| async move {
        let plant = Plant::unserved(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "the disk is full");
        plant.answers.borrow_mut().push_back(failed);
        let mut config = plant.config();
        if blockless {
            config.blocks.clear();
        }
        let run = plant.start(config);
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        for at in 0..6 {
            if at == 3 {
                plant.serve();
            }
            let value = f64::from(u8::try_from(at).expect("fits"));
            write(&mut writer, first + at, &[(value, -value)]);
            plant.clock.sleep(Span::from_nanos(2_000_000_000)).await;
        }
        plant.clock.sleep(Span::MINUTE).await;
        plant.stop(run).await.expect("ok after a cancel");
        let store = plant.store.borrow();
        let points: Vec<_> = (store.points("telemetry", &[]))
            .map(|point| (point.time.nanos(), format!("{:?}", point.fields)))
            .collect();
        let got = plant.got();
        let accepted = (got.iter().skip(1))
            .map(|(_, body)| body.lines().count())
            .sum::<usize>();
        (points, accepted, plant.errors.borrow().clone(), first)
    });
    let expected: Vec<_> = (0..6)
        .map(|at| {
            let fields =
                format!("{{\"edge.a\": Float({at}.0), \"edge.b\": Float(-{at}.0)}}");
            (first + at, fields)
        })
        .collect();
    assert_eq!(points, expected);
    assert_eq!(accepted, expected.len(), "the lines of each 204");
    let refused = "the write to InfluxDB failed: the connect failed: 10.0.0.1:8086 \
                   refused the connection";
    let full = "InfluxDB answered 500 Internal Server Error: the disk is full";
    assert_eq!(errors, ["", refused, full, ""]);
}

#[test]
fn adds_a_field_before_the_fields_of_the_layout() {
    let (got, first) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write_columns(&mut writer, first, &[(3, &[3.0])]);
        write(&mut writer, first + 1, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::SECOND).await;
        plant.stop(run).await.expect("ok after a cancel");
        (plant.got(), first)
    });
    let body = [
        format!("telemetry edge.b=3e0 {first}\n"),
        line("1e0", "2e0", first + 1),
    ];
    let got: Vec<_> = got.into_iter().map(|(_, body)| body).collect();
    assert_eq!(got.concat(), body.concat());
}

/// With no `hold`, a restart loses the batch in flight, as INFLUX KIND states until
/// #274.
#[test]
fn loses_the_batch_in_flight_at_a_restart_with_no_hold() {
    let (first, second) = run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "");
        plant.answers.borrow_mut().extend([failed; 100]);
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        let mut bodies = Vec::new();
        for value in [1.0, 2.0] {
            let mut config = plant.config();
            config.blocks.clear();
            let run = plant.start(config);
            plant.clock.sleep(Span::SECOND).await;
            let first = writer.now().nanos();
            write(&mut writer, first, &[(value, value)]);
            plant.clock.sleep(Span::from_nanos(3_000_000_000)).await;
            plant.stop(run).await.expect("ok after a cancel");
            plant.answers.borrow_mut().clear();
            let got = std::mem::take(&mut *plant.got.borrow_mut());
            bodies.push(got.into_iter().map(|(_, body)| body).collect::<Vec<_>>());
        }
        let second = bodies.pop().expect("two runs");
        (bodies.pop().expect("two runs"), second)
    });
    assert!(first.len() > 1, "{first:?}");
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        second[0].starts_with("telemetry edge.a=2e0,edge.b=2e0 "),
        "{second:?}"
    );
    assert_eq!(second[0].lines().count(), 1, "{second:?}");
}
