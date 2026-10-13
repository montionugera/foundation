use std::collections::BTreeMap;

use document::diagnostic::{Code, Diagnostic};
use hub::reader::{Config, Mode};
use spec::definition::Definition;
use types::name::{Name, Selector};
use types::time::Span;

use super::Reading;
use crate::cancel::Token;
use crate::common::{STATUS, env, run_on};
use crate::kind;
use crate::status::{Class, State, Writer};
use crate::testing::create_status;

fn name(text: &str) -> Name {
    text.parse().expect("a valid name")
}

/// What the status writer does.
enum Step {
    Start,
    /// Sets the count `in`, and writes it.
    Count(u64),
    End(Result<(), kind::Error>),
    Wait,
    Stop,
}

/// The reading of the newest frame of a `Latest` reader of `select`, after the
/// status writer of `plant.modbus`, whose kind names the count `in`, does `steps`.
fn read(steps: Vec<Step>, select: &'static str) -> Option<Reading> {
    run_on(move |node, tasks| async move {
        let (hub, _) = hub::testing::open(env(&node, tasks)).await;
        let connector = name("plant.modbus");
        let counts = vec![name("in")];
        let definitions: BTreeMap<Name, Definition> =
            create_status(&connector, &counts, STATUS)
                .into_iter()
                .collect();
        hub.set_definitions(definitions.iter());
        let clock = node.clock();
        let opened =
            Writer::open(&hub, &connector, counts, clock.clone(), Token::new());
        let (writer, status) = opened.await.expect("the writer opens");
        for step in steps {
            match step {
                Step::Start => assert!(writer.start().await),
                Step::Count(value) => {
                    status.count("in").set(value);
                    writer.during(clock.sleep(Span::SECOND)).await;
                }
                Step::End(end) => writer.end(&end).await,
                Step::Wait => {
                    writer.wait(Span::SECOND).await;
                }
                Step::Stop => writer.stop().await,
            }
        }
        let config = Config {
            select: Selector::new([select]).expect("a selector"),
            mode: Mode::Latest,
            subject: name("admin"),
            name: None,
            hold: Span::ZERO,
        };
        let mut reader = hub.reader(config).await.expect("the reader opens");
        let received = reader.next().await.expect("the newest frame");
        let names = definitions
            .iter()
            .filter_map(|(name, definition)| match definition {
                Definition::Channel(channel) => Some((channel.key, name.clone())),
                _ => None,
            })
            .collect();
        Reading::of(&connector, &received, &names)
    })
}

/// A reading with the count `in` at `value`.
fn reading(
    state: State,
    restarts: u64,
    error: Option<(Class, &str)>,
    value: u64,
) -> Reading {
    Reading {
        state,
        restarts,
        error: error.map(|(class, text)| (class, text.to_owned())),
        counts: BTreeMap::from([(name("in"), value)]),
    }
}

fn device() -> Step {
    Step::End(Err(kind::Error::Device("busy".into())))
}

const ALL: &str = "plant.modbus.status.*";

#[test]
fn reads_a_running_connector_and_its_counts() {
    let got = read(vec![Step::Start, Step::Count(7)], ALL);
    assert_eq!(got, Some(reading(State::Running, 0, None, 7)));
}

#[test]
fn reads_the_error_of_a_run_that_ends() {
    let got = read(vec![Step::Start, device()], ALL);
    let want = reading(State::Ending, 0, Some((Class::Device, "busy")), 0);
    assert_eq!(got, Some(want));
}

#[test]
fn reads_a_connector_that_restarts() {
    let got = read(vec![Step::Start, device(), Step::Wait], ALL);
    let want = reading(State::Restarting, 0, Some((Class::Device, "busy")), 0);
    assert_eq!(got, Some(want));
}

#[test]
fn reads_the_restarts_and_keeps_the_last_error_after_a_restart() {
    let got = read(vec![Step::Start, device(), Step::Wait, Step::Start], ALL);
    let want = reading(State::Running, 1, Some((Class::Device, "busy")), 0);
    assert_eq!(got, Some(want));
}

#[test]
fn reads_a_stopped_connector_whose_last_run_returned_ok() {
    let got = read(vec![Step::Start, Step::End(Ok(())), Step::Stop], ALL);
    assert_eq!(got, Some(reading(State::Stopped, 0, None, 0)));
}

#[test]
fn reads_the_class_of_a_config_and_a_retry_error() {
    let bad = Diagnostic::new(
        Code::new("test.bad"),
        None,
        "the rate is bad".into(),
        "Fix it".into(),
    );
    let text = bad.to_string();
    let config = Step::End(Err(kind::Error::Config(vec![bad])));
    let got = read(vec![Step::Start, config], ALL);
    let want = reading(State::Ending, 0, Some((Class::Config, &text)), 0);
    assert_eq!(got, Some(want));
    let retry = Step::End(Err(kind::Error::Retry("again".into())));
    let got = read(vec![Step::Start, retry], ALL);
    let want = reading(State::Ending, 0, Some((Class::Retry, "again")), 0);
    assert_eq!(got, Some(want));
}

#[test]
fn gives_none_for_a_frame_with_no_channel_of_the_supervisor() {
    let got = read(vec![Step::Start], "plant.modbus.status.in");
    assert_eq!(got, None);
}

#[test]
fn gives_none_for_a_value_that_no_state_or_class_has() {
    assert_eq!(State::of(4), None);
    assert_eq!(Class::of(4), None);
    assert_eq!(Class::of(0), None);
}

#[test]
fn words_each_state_and_class() {
    let states = [
        State::Running,
        State::Restarting,
        State::Stopped,
        State::Ending,
    ];
    let words: Vec<_> = states.iter().map(ToString::to_string).collect();
    assert_eq!(words, ["running", "restarting", "stopped", "ending"]);
    let classes = [Class::Config, Class::Device, Class::Retry];
    let words: Vec<_> = classes.iter().map(ToString::to_string).collect();
    assert_eq!(words, ["config", "device", "retry"]);
}
