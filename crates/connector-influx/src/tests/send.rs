use std::cell::RefCell;
use std::collections::VecDeque;
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
/// The key of the first status channel of `influx`.
const STATUS: u128 = 100;

/// One request that the InfluxDB of a plant got: its path and query, and its body.
type Got = (String, String);

/// The end of one supervised run, once it returned.
type End = Rc<RefCell<Option<Result<(), Error>>>>;

/// A node with a hub that defines `edge.time` (key 1), the `f64` channels `edge.a`
/// (2) and `edge.b` (3), the `i64` channel `edge.count` (4), the `f64` channel `time`
/// (5), and the status channels of `influx`, and an InfluxDB that answers each
/// request with the next answer of its list. After the last, it writes the body to its
/// store, and answers 204, or 400 when the store refuses a line.
struct Plant {
    hub: hub::Hub,
    supervisor: Supervisor,
    clock: Clock,
    net: env::net::Net,
    tasks: Tasks,
    address: SocketAddr,
    got: Rc<RefCell<Vec<Got>>>,
    answers: Rc<RefCell<VecDeque<(StatusCode, &'static str)>>>,
    store: Rc<RefCell<crate::sim::Store>>,
    errors: Rc<RefCell<Vec<String>>>,
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
        define(&inputs);
        let errors = read_errors(&inputs.hub, &tasks).await;
        Self {
            hub: inputs.hub.clone(),
            supervisor: Supervisor::new(inputs),
            clock: node.clock(),
            net: node.net(),
            tasks,
            address,
            got: Rc::default(),
            answers: Rc::default(),
            store: Rc::default(),
            errors,
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
        let answer = move |request: Request<Bytes>| {
            let body = String::from_utf8(request.body().to_vec()).expect("UTF-8");
            got.borrow_mut().push((request.uri().to_string(), body));
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
    let data = |key, scalar| {
        let data_type = DataType::Sample(Type::Scalar(scalar));
        let data = Data::new(time, None, data_type, None).expect("no unit");
        let key = channel::Key::from_u128(key);
        let kind = spec::channel::Kind::Data(data);
        Definition::Channel(Channel { key, kind })
    };
    let index = spec::channel::Kind::Index {
        error: None,
        control: None,
    };
    let mut definitions =
        testing::create_status(&name("influx"), &[], channel::Key::from_u128(STATUS));
    definitions.extend([
        (
            name("edge.time"),
            Definition::Channel(Channel {
                key: time,
                kind: index,
            }),
        ),
        (name("edge.a"), data(2, Scalar::F64)),
        (name("edge.b"), data(3, Scalar::F64)),
        (name("edge.count"), data(4, Scalar::I64)),
        (name("time"), data(5, Scalar::F64)),
    ]);
    inputs
        .hub
        .set_definitions(definitions.iter().map(|(name, def)| (name, def)));
}

/// Reads each text of `influx.status.error`, in a task on `tasks`, into the vector
/// it gives, with no text equal to the one before it.
async fn read_errors(hub: &hub::Hub, tasks: &Tasks) -> Rc<RefCell<Vec<String>>> {
    let open = hub::reader::Config {
        select: Selector::new(["influx.status.error"]).expect("a selector"),
        mode: Mode::Complete,
        subject: name("test"),
        name: None,
        hold: Span::ZERO,
    };
    let mut reader = hub.reader(open).await.expect("the reader opens");
    let errors = Rc::new(RefCell::new(Vec::new()));
    let into = Rc::clone(&errors);
    tasks.spawn(async move {
        let text = codec::Variable::of(Type::String).expect("a variable type");
        let mut raw = Vec::new();
        while let Ok(received) = reader.next().await {
            let (view, set) = (received.view(), received.set());
            for (entry, bytes) in view.iter() {
                let entry = &set.entries()[entry];
                if entry.data_type != Type::String {
                    continue;
                }
                let range = view.range(entry.group).expect("a range");
                let count = usize::try_from(range.count).expect("fits");
                codec::decode(entry.data_type, count, bytes, &mut raw)
                    .expect("decodes");
                let mut into = into.borrow_mut();
                for sample in text.samples(count, &raw) {
                    let sample = std::str::from_utf8(sample).expect("UTF-8");
                    if into.last().is_none_or(|last: &String| last != sample) {
                        into.push(sample.to_owned());
                    }
                }
            }
        }
    });
    errors
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
    let entries = writer.set().entries();
    let at = |key| {
        let key = channel::Key::from_u128(key);
        entries
            .iter()
            .position(|entry| entry.key == key)
            .expect("written")
    };
    let count = columns.first().map_or(0, |(_, samples)| samples.len());
    let time = at(1);
    let group = entries[time].group;
    let mut series: Vec<_> = std::iter::once(time)
        .chain(columns.iter().map(|&(key, _)| at(key)))
        .map(|entry| (entry, count * 8))
        .collect();
    series.sort_unstable();
    let mut draft = writer.draft(Form::Raw, &series).expect("a frame");
    let times = (first..).take(count).flat_map(i64::to_le_bytes);
    fill(draft.series_mut(time).expect("the index"), times);
    for &(key, samples) in columns {
        let bytes = samples.iter().flat_map(|sample| sample.to_le_bytes());
        fill(draft.series_mut(at(key)).expect("a series"), bytes);
    }
    draft.set_count(group, u32::try_from(count).expect("fits"));
    let outcomes = writer.write(Label::Path(Path::Live), draft);
    let applied = outcomes.is_ok_and(|outcomes| {
        (outcomes.iter())
            .all(|outcome| matches!(outcome, hub::home::Outcome::Applied { .. }))
    });
    assert!(applied, "the frame applies");
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
fn sends_a_batch_again_after_each_other_answer_and_shows_the_error_until_a_204() {
    let (got, errors) = run(|node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        plant.answers.borrow_mut().extend([
            (StatusCode::INTERNAL_SERVER_ERROR, " the disk is full \n"),
            (StatusCode::NOT_FOUND, "database not found: \"plant\""),
            (StatusCode::OK, "ok"),
        ]);
        let run = plant.start(plant.config());
        let mut writer = plant.writer(&["edge.time", "edge.a", "edge.b"]).await;
        plant.clock.sleep(Span::SECOND).await;
        let first = writer.now().nanos();
        write(&mut writer, first, &[(1.0, 2.0)]);
        plant.clock.sleep(Span::from_nanos(20_000_000_000)).await;
        plant.stop(run).await.expect("ok after a cancel");
        let got = (plant.got().into_iter())
            .map(|(uri, body)| (uri, body == line("1e0", "2e0", first)))
            .collect::<Vec<_>>();
        (got, plant.errors.borrow().clone())
    });
    assert_eq!(got, vec![(WRITE.to_owned(), true); 4]);
    assert_eq!(
        errors,
        [
            "",
            "InfluxDB answered 500 Internal Server Error: the disk is full",
            "InfluxDB answered 404 Not Found: database not found: \"plant\"",
            "InfluxDB answered 200 OK: ok",
            "",
        ]
    );
}

/// The bodies that the InfluxDB of a plant gets in two runs, one after the other, and
/// the errors that the second run shows. InfluxDB answers each request of the first
/// run with 500, or with 204 when `accepted`, and each of the second with 204. Each
/// run writes one frame of one sample time, at which each channel holds its value of
/// `values`.
fn two_runs(
    accepted: bool,
    values: [f64; 2],
) -> (Vec<String>, Vec<String>, Vec<String>) {
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
        let second = bodies.pop().expect("two runs");
        (bodies.pop().expect("two runs"), second, errors)
    })
}

#[test]
fn acks_no_batch_before_a_204() {
    let (first, second, errors) = two_runs(false, [1.0, 2.0]);
    assert!(first.len() > 1, "{first:?}");
    let sent = first.first().expect("a request");
    assert!(first.iter().all(|body| body == sent), "{first:?}");
    assert!(
        sent.starts_with("telemetry edge.a=1e0,edge.b=1e0 "),
        "{sent}"
    );
    // Known bug #2553: each open after a lost batch ends `Behind`, so the second run
    // sends nothing.
    assert_eq!(second, Vec::<String>::new());
    let missed = "the reader missed a frame and gets no later one: open a new reader";
    assert!(errors.iter().any(|error| error == missed), "{errors:?}");
}

#[test]
fn acks_a_batch_after_its_204() {
    let (first, second, errors) = two_runs(true, [1.0, 2.0]);
    assert_eq!(first.len(), 1, "{first:?}");
    assert!(first[0].starts_with("telemetry edge.a=1e0,edge.b=1e0 "));
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(second[0].starts_with("telemetry edge.a=2e0,edge.b=2e0 "));
    assert_eq!(errors, [""]);
}

#[test]
fn acks_a_batch_of_no_line_with_no_request() {
    let (first, second, errors) = two_runs(true, [f64::NAN, 2.0]);
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

/// The end of a run with `config` that gets one frame of the channel `channel`, of
/// key `key`, which holds 8-byte samples, and the requests that InfluxDB gets.
fn end_of_one_frame(
    select: &'static str,
    channel: &'static str,
    key: u128,
) -> (Option<Result<(), Error>>, Vec<Got>) {
    run(move |node, tasks| async move {
        let plant = Plant::new(node, tasks).await;
        let (_, end) = plant.start(with(plant.config(), "select", &string(select)));
        let mut writer = plant.writer(&["edge.time", channel]).await;
        plant.clock.sleep(Span::SECOND).await;
        let entries = writer.set().entries();
        let at = |key| {
            let key = channel::Key::from_u128(key);
            entries
                .iter()
                .position(|entry| entry.key == key)
                .expect("written")
        };
        let (time, value) = (at(1), at(key));
        let group = entries[time].group;
        let mut series = [(time, 8), (value, 8)];
        series.sort_unstable();
        let mut draft = writer.draft(Form::Raw, &series).expect("a frame");
        let stamp = writer.now().nanos();
        fill(
            draft.series_mut(time).expect("the index"),
            stamp.to_le_bytes().into_iter(),
        );
        fill(
            draft.series_mut(value).expect("the value"),
            7_i64.to_le_bytes().into_iter(),
        );
        draft.set_count(group, 1);
        writer
            .write(Label::Path(Path::Live), draft)
            .expect("the home takes it");
        plant.clock.sleep(Span::SECOND).await;
        let end = end.borrow_mut().take();
        (end, plant.got())
    })
}

#[test]
fn stops_with_a_config_error_at_a_channel_that_does_not_hold_f64() {
    let (end, got) = end_of_one_frame("edge.*", "edge.count", 4);
    assert_eq!(got, []);
    let Some(Err(Error::Config(diagnostics))) = end else {
        panic!("a config error, not {end:?}");
    };
    assert_eq!(
        diagnostics,
        [Diagnostic::new(
            Code::new("influx.sample-type"),
            None,
            "the channel \"edge.count\" holds i64 samples, which the influx kind does \
             not write"
                .into(),
            "Select only f64 channels".into(),
        )]
    );
}

#[test]
fn stops_with_a_config_error_at_a_channel_whose_name_is_no_field_key() {
    let (end, got) = end_of_one_frame("time", "time", 5);
    assert_eq!(got, []);
    let Some(Err(Error::Config(diagnostics))) = end else {
        panic!("a config error, not {end:?}");
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
    stores_each_sample_once_after_writes_that_fail(true);
}

#[test]
fn holds_each_sample_after_writes_that_fail_with_no_reader_block() {
    stores_each_sample_once_after_writes_that_fail(false);
}

/// Checks that InfluxDB stores each sample once, in time order, when it refuses each
/// connection for 6 s, then answers 500 once. The config has the `reader` block of
/// [`config`] when `reader`, else none.
fn stores_each_sample_once_after_writes_that_fail(reader: bool) {
    let (points, accepted, errors, first) = run(move |node, tasks| async move {
        let plant = Plant::unserved(node, tasks).await;
        let failed = (StatusCode::INTERNAL_SERVER_ERROR, "the disk is full");
        plant.answers.borrow_mut().push_back(failed);
        let mut config = plant.config();
        if !reader {
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
