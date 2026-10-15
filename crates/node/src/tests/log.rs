use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use proptest::prelude::*;
use serde_json::{Value, json};
use types::byte::Size;
use types::time::Span;

use super::{config, heap};
use crate::{Config, Error, Log, Node, log};

/// The lines a log writes, which each clone shares.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<u8>>>);

impl Lines {
    /// Each line written, without its newline.
    fn read(&self) -> Vec<String> {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let text = String::from_utf8(bytes.clone()).expect("a log writes UTF-8");
        text.lines().map(str::to_owned).collect()
    }
}

impl io::Write for Lines {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut lines = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        lines.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer whose each write fails.
struct Broken;

impl io::Write for Broken {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("the pipe broke"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The log of `json` lines of `level` and above to `lines`.
fn to(lines: &Lines, json: bool, level: tracing::Level) -> Log {
    Log {
        writer: Box::new(lines.clone()),
        format: ops::Log { json, level },
    }
}

/// Starts a node on `host` with `log`, runs `sim` for a second, then stops the node
/// and gives its join.
fn run(sim: &mut sim::Sim, host: &sim::node::Node, log: Log) -> Result<(), Error> {
    let node = Node::start(Config {
        log,
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    node.join()
}

/// A sim of `seed` with one host.
fn host(seed: u64) -> (sim::Sim, sim::node::Node) {
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let host = sim.node(sim::node::Config::default());
    (sim, host)
}

#[test]
fn a_node_logs_its_start_and_its_stop_in_text() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = to(&lines, false, tracing::Level::INFO);
    assert_eq!(run(&mut sim, &host, log), Ok(()));
    assert_eq!(
        lines.read(),
        [
            "2026-01-01T00:00:00.004198Z INFO node: started name=edge \
             address=10.0.0.1:7000 shards=4",
            "2026-01-01T00:00:01.000004Z INFO node: stopped",
        ]
    );
}

#[test]
fn a_node_logs_its_start_and_its_stop_in_json() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = to(&lines, true, tracing::Level::INFO);
    assert_eq!(run(&mut sim, &host, log), Ok(()));
    assert_eq!(
        lines.read(),
        [
            concat!(
                r#"{"timestamp":"2026-01-01T00:00:00.004198Z","level":"INFO","#,
                r#""target":"node","fields":{"message":"started","name":"edge","#,
                r#""address":"10.0.0.1:7000","shards":4}}"#,
            ),
            concat!(
                r#"{"timestamp":"2026-01-01T00:00:01.000004Z","level":"INFO","#,
                r#""target":"node","fields":{"message":"stopped"}}"#,
            ),
        ]
    );
}

#[test]
fn a_node_that_fails_at_its_start_logs_the_error_with_no_mesh_time() {
    let (_sim, host) = host(1);
    let error = "the name @edge has a segment that starts with `@`, which is reserved \
                 for Foundation; give another name";
    let text = format!(
        "2026-01-01T00:00:00.000000Z ERROR node: stopped error=\"{error}\" mesh_time=none"
    );
    let json = format!(
        concat!(
            r#"{{"timestamp":"2026-01-01T00:00:00.000000Z","level":"ERROR","#,
            r#""target":"node","fields":{{"message":"stopped","error":"{error}"}},"#,
            r#""mesh_time":null}}"#,
        ),
        error = error,
    );
    for (json, line) in [(false, text), (true, json)] {
        let lines = Lines::default();
        let node = Node::start(Config {
            name: "@edge".parse().expect("a name"),
            log: to(&lines, json, tracing::Level::INFO),
            ..config(&host, Size::MEBIBYTE, Box::new(heap))
        });
        assert!(matches!(node.join(), Err(Error::Reserved { .. })));
        assert_eq!(lines.read(), [line], "json: {json}");
    }
}

#[test]
fn a_wall_step_shows_mesh_time_beside_the_stamp() {
    let text = [
        "2026-01-01T00:00:00.004198Z INFO node: started name=edge address=10.0.0.1:7000 \
         shards=4",
        "2026-01-01T00:00:03.000004Z INFO node: stopped \
         mesh_time=2026-01-01T00:00:01.000004Z",
    ];
    let json = [
        concat!(
            r#"{"timestamp":"2026-01-01T00:00:00.004198Z","level":"INFO","#,
            r#""target":"node","fields":{"message":"started","name":"edge","#,
            r#""address":"10.0.0.1:7000","shards":4}}"#,
        ),
        concat!(
            r#"{"timestamp":"2026-01-01T00:00:03.000004Z","level":"INFO","#,
            r#""target":"node","fields":{"message":"stopped"},"#,
            r#""mesh_time":"2026-01-01T00:00:01.000004Z"}"#,
        ),
    ];
    for (json, written) in [(false, text), (true, json)] {
        let (mut sim, host) = host(1);
        let lines = Lines::default();
        let node = Node::start(Config {
            log: to(&lines, json, tracing::Level::INFO),
            ..config(&host, Size::MEBIBYTE, Box::new(heap))
        });
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        host.step_wall(Span::from_nanos(2_000_000_000));
        node.stop();
        assert_eq!(sim.run(), Ok(()));
        assert_eq!(node.join(), Ok(()));
        assert_eq!(lines.read(), written, "json: {json}");
    }
}

#[test]
fn each_node_writes_only_its_own_log() {
    let (mut sim, a) = host(1);
    let b = sim.node(sim::node::Config::default());
    let (to_a, to_b) = (Lines::default(), Lines::default());
    let start = |host: &sim::node::Node, name: &str, lines: &Lines| {
        Node::start(Config {
            name: name.parse().expect("a name"),
            log: to(lines, false, tracing::Level::INFO),
            ..config(host, Size::MEBIBYTE, Box::new(heap))
        })
    };
    let nodes = [start(&a, "a", &to_a), start(&b, "b", &to_b)];
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    for node in &nodes {
        node.stop();
    }
    assert_eq!(sim.run(), Ok(()));
    for node in nodes {
        assert_eq!(node.join(), Ok(()));
    }
    let started = |lines: &Lines| {
        let lines = lines.read();
        let line = lines.first().expect("a started line").clone();
        line.split_once(" INFO ").expect("a level").1.to_owned()
    };
    assert_eq!(
        (started(&to_a), started(&to_b)),
        (
            "node: started name=a address=10.0.0.1:7000 shards=4".to_owned(),
            "node: started name=b address=10.0.0.2:7000 shards=4".to_owned(),
        )
    );
}

#[test]
fn a_level_above_info_drops_the_start_and_the_stop() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = to(&lines, false, tracing::Level::WARN);
    assert_eq!(run(&mut sim, &host, log), Ok(()));
    assert_eq!(lines.read(), Vec::<String>::new());
}

#[test]
fn a_log_whose_writes_fail_changes_nothing_of_the_node() {
    let (mut sim, host) = host(1);
    let log = Log {
        writer: Box::new(Broken),
        format: ops::Log {
            json: false,
            level: tracing::Level::TRACE,
        },
    };
    assert_eq!(run(&mut sim, &host, log), Ok(()));
}

/// What a log of `json` lines writes for the events of `events`, on a host whose
/// mesh clock has not synced.
fn written(json: bool, events: impl FnOnce()) -> String {
    let (_sim, host) = host(1);
    let (_, mesh) = clock::Clock::new(host.clock());
    let lines = Lines::default();
    let log = to(&lines, json, tracing::Level::INFO);
    let dispatch = log::dispatch(log, host.wall(), mesh);
    tracing::dispatcher::with_default(&dispatch, events);
    let bytes = lines.0.lock().unwrap_or_else(PoisonError::into_inner);
    String::from_utf8(bytes.clone()).expect("a log writes UTF-8")
}

/// The line of an event with `field` and other fields.
fn line(json: bool, field: &str) -> String {
    written(json, || {
        tracing::info!(target: "test", field, count = 3_u64, delta = -2_i64, ok = true, "a\nb");
    })
}

#[test]
fn a_newline_and_a_backslash_in_a_field_stay_on_one_line() {
    assert_eq!(
        line(false, "x\ny\\z\"q"),
        concat!(
            r#"2026-01-01T00:00:00.000000Z INFO test: a\nb field="x\ny\\z\"q" count=3 "#,
            "delta=-2 ok=true mesh_time=none\n",
        )
    );
    assert_eq!(
        line(true, "x\ny\\z\"q"),
        concat!(
            r#"{"timestamp":"2026-01-01T00:00:00.000000Z","level":"INFO","#,
            r#""target":"test","fields":{"message":"a\nb","field":"x\ny\\z\"q","#,
            r#""count":3,"delta":-2,"ok":true},"mesh_time":null}"#,
            "\n",
        )
    );
}

#[test]
fn a_text_value_is_quoted_only_when_it_must_be() {
    for (field, written) in [
        ("plain", "plain"),
        ("", r#""""#),
        ("a b", r#""a b""#),
        ("a=b", r#""a=b""#),
        ("it's", "it's"),
        ("\t", r#""\t""#),
        ("\u{1b}", r#""\u{1b}""#),
    ] {
        let line = written_field(field);
        assert_eq!(line, written, "{field:?}");
    }
}

/// The text of the one field `field` of an event with no message.
fn written_field(field: &str) -> String {
    let line = written(false, || tracing::info!(target: "test", field));
    let rest = line.strip_prefix("2026-01-01T00:00:00.000000Z INFO test: field=");
    let rest = rest.and_then(|rest| rest.strip_suffix(" mesh_time=none\n"));
    rest.unwrap_or_else(|| panic!("not a line of one field: {line:?}"))
        .to_owned()
}

/// The string that `text` escapes as `char::escape_debug` does, for the escapes that
/// the log writes, else `None`.
fn unescape(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match chars.next()? {
            '\\' => '\\',
            '"' => '"',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '0' => '\0',
            'u' => {
                let hex: String =
                    chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
            }
            _ => return None,
        });
    }
    Some(out)
}

#[test]
fn a_json_line_gives_a_128_bit_integer_as_a_number() {
    let line = written(true, || {
        tracing::info!(target: "test", big = 5_u128, small = -5_i128);
    });
    let line: Value = serde_json::from_str(&line).expect("JSON");
    assert_eq!(line["fields"], json!({ "big": 5, "small": -5 }));
}

proptest! {
    #[test]
    fn a_text_message_is_one_line_that_reads_back(message in any::<String>()) {
        let line = written(false, || tracing::info!(target: "test", "{message}"));
        let rest = line.strip_prefix("2026-01-01T00:00:00.000000Z INFO test: ");
        let rest = rest.and_then(|rest| rest.strip_suffix(" mesh_time=none\n"));
        let rest = rest.expect("one line");
        prop_assert!(!rest.contains('\n'), "{line:?}");
        prop_assert_eq!(unescape(rest), Some(message));
    }

    #[test]
    fn a_text_value_is_one_value_that_reads_back(field in any::<String>()) {
        let written = written_field(&field);
        prop_assert!(!written.contains('\n'), "{written:?}");
        let quoted = written.strip_prefix('"').and_then(|w| w.strip_suffix('"'));
        let read = if let Some(quoted) = quoted {
            let open = quoted.replace("\\\\", "").replace("\\\"", "");
            prop_assert!(!open.contains('"'), "an unescaped quote: {written:?}");
            unescape(quoted)
        } else {
            prop_assert!(!written.contains([' ', '=', '"', '\\']), "{written:?}");
            Some(written.clone())
        };
        prop_assert_eq!(read, Some(field));
    }

    #[test]
    fn a_json_line_gives_back_each_value(field in any::<String>()) {
        let line = line(true, &field);
        prop_assert_eq!(line.matches('\n').count(), 1);
        let line: Value = serde_json::from_str(&line).expect("JSON");
        prop_assert_eq!(&line["fields"]["field"], &json!(field));
    }
}
