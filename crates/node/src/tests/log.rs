use std::io;
use std::panic;
use std::process::Command;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::Duration;

use proptest::prelude::*;
use serde_json::{Value, json};
use types::byte::Size;
use types::time::Span;

use super::{config, heap};
use crate::{Config, Error, Flush, Log, Node, log};

/// The variable that marks a child process of a test.
const CHILD: &str = "NODE_LOG_CHILD";

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

/// A writer that panics on each line that holds `boom`.
struct Panics(Lines);

impl io::Write for Panics {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        assert!(
            !bytes.windows(4).any(|word| word == b"boom"),
            "the writer panics"
        );
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A writer whose first write blocks until `free` gets a message or drops, as a
/// reader of stderr that stops. It sends on `entered` when it blocks.
struct Stuck {
    lines: Lines,
    entered: Option<mpsc::Sender<()>>,
    free: mpsc::Receiver<()>,
}

impl io::Write for Stuck {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).unwrap_or_default();
            #[expect(clippy::disallowed_methods, reason = "a writer that blocks")]
            self.free.recv().unwrap_or_default();
        }
        self.lines.write(bytes)
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
/// and gives its join, once the log has written `stopped`.
fn run(sim: &mut sim::Sim, host: &sim::node::Node, log: Log) -> Result<(), Error> {
    let node = Node::start(Config {
        log,
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    let joined = node.join();
    assert_eq!(sim.run(), Ok(()));
    joined
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
    let (mut sim, host) = host(1);
    let error = "the name @edge has a segment that starts with `@`, which is reserved \
                 for Foundation; give another name";
    let text = format!(
        "2026-01-01T00:00:00.000000Z ERROR node: stopped error=\"{error}\" \
         mesh_time=none"
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
        assert_eq!(sim.run(), Ok(()));
        assert_eq!(lines.read(), [line], "json: {json}");
    }
}

#[test]
fn a_wall_step_shows_mesh_time_beside_the_stamp() {
    let text = [
        "2026-01-01T00:00:00.004198Z INFO node: started name=edge \
         address=10.0.0.1:7000 shards=4",
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
        assert_eq!(sim.run(), Ok(()));
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
    assert_eq!(sim.run(), Ok(()));
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

/// The log of `json` lines at INFO to `writer` on `host`, whose mesh clock has not
/// synced, with no thread `log` yet.
fn opened(
    host: &sim::node::Node,
    json: bool,
    writer: Box<dyn io::Write + Send>,
) -> Flush {
    let (_, mesh) = clock::Clock::new(host.clock());
    let format = ops::Log {
        json,
        level: tracing::Level::INFO,
    };
    log::open(Log { writer, format }, &host.threads(), host.wall(), mesh)
}

/// Puts the lines of `events` in the queue of `log`, then writes them in `sim`.
fn write(sim: &mut sim::Sim, log: &Flush, events: impl FnOnce()) {
    tracing::dispatcher::with_default(&log.dispatch, events);
    assert!(log.resume(), "the thread log starts");
    assert_eq!(sim.run(), Ok(()));
}

/// What a log of `json` lines writes for the events of `events`, on a host whose
/// mesh clock has not synced.
fn written(json: bool, events: impl FnOnce()) -> String {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = opened(&host, json, Box::new(lines.clone()));
    write(&mut sim, &log, events);
    let bytes = lines.0.lock().unwrap_or_else(PoisonError::into_inner);
    String::from_utf8(bytes.clone()).expect("a log writes UTF-8")
}

/// The events `test` with the field `n` for each of `from..to`.
fn numbered(from: u64, to: u64) -> impl FnOnce() {
    move || {
        for n in from..to {
            tracing::info!(target: "test", n);
        }
    }
}

/// The line of the event `n` of [`numbered`] at the start of the sim.
fn number(n: u64) -> String {
    format!("2026-01-01T00:00:00.000000Z INFO test: n={n} mesh_time=none")
}

#[test]
fn a_full_queue_drops_each_new_line_and_counts_them_after_the_lines_before() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = opened(&host, false, Box::new(lines.clone()));
    write(&mut sim, &log, numbered(0, 1027));
    write(&mut sim, &log, numbered(1027, 1028));
    let mut written: Vec<String> = (0..1024).map(number).collect();
    written.push(
        "2026-01-01T00:00:00.000000Z WARN node::log: dropped lines=3 mesh_time=none"
            .to_owned(),
    );
    written.push(number(1027));
    assert_eq!(lines.read(), written);
}

#[test]
fn a_write_that_panics_loses_only_its_line() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let log = opened(&host, false, Box::new(Panics(lines.clone())));
    write(&mut sim, &log, || {
        tracing::info!(target: "test", "a");
        tracing::info!(target: "test", "boom");
        tracing::info!(target: "test", "c");
    });
    assert_eq!(
        lines.read(),
        [
            "2026-01-01T00:00:00.000000Z INFO test: a mesh_time=none",
            "2026-01-01T00:00:00.000000Z INFO test: c mesh_time=none",
        ]
    );
}

#[test]
fn a_line_after_the_shards_end_comes_before_stopped() {
    let (mut sim, host) = host(1);
    let lines = Lines::default();
    let node = Node::start(Config {
        log: to(&lines, false, tracing::Level::INFO),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    let late = || tracing::info!(target: "test", "late");
    tracing::dispatcher::with_default(&node.flush().dispatch, late);
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(lines.read().len(), 1, "the line waits for the join");
    assert_eq!(node.join(), Ok(()));
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(
        lines.read(),
        [
            "2026-01-01T00:00:00.004198Z INFO node: started name=edge \
             address=10.0.0.1:7000 shards=4",
            "2026-01-01T00:00:01.000004Z INFO test: late",
            "2026-01-01T00:00:01.000004Z INFO node: stopped",
        ]
    );
}

/// The lines of `lines` with no stamp.
fn unstamped<'a>(lines: &'a [String]) -> Vec<&'a str> {
    let unstamp = |line: &'a String| line.split_once(' ').expect("a stamp").1;
    lines.iter().map(unstamp).collect()
}

/// The log of a writer that blocks at its first line, on OS threads, with its thread
/// `log` running. Gives the log, its hold, what the writer wrote, the receiver of
/// the block of the writer, and the sender that frees it.
fn stuck() -> (
    Flush,
    log::Hold,
    Lines,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
) {
    let lines = Lines::default();
    let (entered, blocked) = mpsc::channel();
    let (free, freed) = mpsc::channel();
    let writer = Stuck {
        lines: lines.clone(),
        entered: Some(entered),
        free: freed,
    };
    let format = ops::Log {
        json: false,
        level: tracing::Level::INFO,
    };
    let (_, mesh) = clock::Clock::new(os::clock());
    let threads = os::threads().expect("threads");
    let wall = os::wall().expect("a wall clock");
    let log = Log {
        writer: Box::new(writer),
        format,
    };
    let log = log::open(log, &threads, wall, mesh);
    let hold = log.hold().expect("the thread log starts");
    (log, hold, lines, blocked, free)
}

#[test]
fn an_event_never_waits_on_a_stuck_writer() {
    let (log, hold, lines, blocked, free) = stuck();
    tracing::dispatcher::with_default(&log.dispatch, numbered(0, 1));
    #[expect(clippy::disallowed_methods, reason = "the test waits for an OS thread")]
    blocked.recv().expect("the writer blocks");
    tracing::dispatcher::with_default(&log.dispatch, numbered(1, 1030));
    assert!(!log.wait(Duration::from_millis(10)));
    free.send(()).expect("the writer waits");
    drop(hold);
    assert!(log.wait(Duration::from_secs(60)));
    let mut written: Vec<String> = (0..1025)
        .map(|n| format!("INFO test: n={n} mesh_time=none"))
        .collect();
    written.push("WARN node::log: dropped lines=5 mesh_time=none".to_owned());
    assert_eq!(unstamped(&lines.read()), written);
}

/// Runs the test `name` again in a child process with `CHILD` set to `case`, and
/// gives its stderr.
fn child(name: &str, case: &str) -> String {
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, case)
        .output()
        .expect("the child runs");
    assert_eq!(
        output.status.code(),
        Some(101),
        "the child panics with no abort"
    );
    String::from_utf8(output.stderr).expect("UTF-8")
}

/// A value whose `Display` panics.
struct Boom;

impl std::fmt::Display for Boom {
    fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        panic!("the field panics");
    }
}

/// What the child of a panic test runs.
#[derive(Clone, Copy)]
enum Case {
    /// The log writes to stderr.
    Free,
    /// The writer blocks.
    Stuck,
    /// The log drops its hold, then panics in an event.
    Late,
    /// The writer panics at one line, and the log waits for the next.
    Writes,
}

/// In a child process: a log with its thread `log` held and each line of
/// `numbered(0, 3)` in its queue, and the panic hook of `main`, then a panic, as
/// `case` says.
fn panics(case: Case) -> ! {
    let (_, mesh) = clock::Clock::new(os::clock());
    let (_free, freed) = mpsc::channel();
    let (entered, _blocked) = mpsc::channel();
    let writer: Box<dyn io::Write + Send> = match case {
        Case::Stuck => Box::new(Stuck {
            lines: Lines::default(),
            entered: Some(entered),
            free: freed,
        }),
        Case::Writes => Box::new(Panics(Lines::default())),
        Case::Free | Case::Late => Box::new(io::stderr()),
    };
    let format = ops::Log {
        json: false,
        level: tracing::Level::INFO,
    };
    let threads = os::threads().expect("threads");
    let wall = os::wall().expect("a wall clock");
    let log = log::open(Log { writer, format }, &threads, wall, mesh);
    let hold = log.hold().expect("the thread log starts");
    tracing::dispatcher::with_default(&log.dispatch, numbered(0, 3));
    let flush = log.clone();
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if log.panicked(info) {
            previous(info);
        }
    }));
    match case {
        Case::Late => {
            drop(hold);
            tracing::dispatcher::with_default(&flush.dispatch, || {
                tracing::info!(target: "test", field = %Boom);
            });
        }
        Case::Writes => {
            tracing::dispatcher::with_default(&flush.dispatch, || {
                tracing::info!(target: "test", "boom");
                tracing::info!(target: "test", "c");
            });
            let bound = Duration::from_millis(500);
            assert!(flush.wait(bound), "the line after the panic is late");
        }
        Case::Free | Case::Stuck => {}
    }
    panic!("the test panics");
}

#[test]
fn a_panic_writes_the_queued_lines_then_the_panic() {
    #[expect(clippy::disallowed_methods, reason = "the parent test marks its child")]
    match std::env::var(CHILD).as_deref() {
        Ok("free") => panics(Case::Free),
        Ok("stuck") => panics(Case::Stuck),
        Ok("late") => panics(Case::Late),
        Ok("writes") => panics(Case::Writes),
        _ => {}
    }
    let name = "tests::log::a_panic_writes_the_queued_lines_then_the_panic";
    let stderr = child(name, "free");
    let (logged, hook) = stderr
        .split_once("\nthread '")
        .expect("the hook of the test harness writes after the log");
    let lines: Vec<String> = logged.lines().map(str::to_owned).collect();
    let lines = unstamped(&lines);
    assert_eq!(
        lines[..3],
        [
            "INFO test: n=0 mesh_time=none",
            "INFO test: n=1 mesh_time=none",
            "INFO test: n=2 mesh_time=none",
        ],
        "{stderr}"
    );
    let at = r#"ERROR node: panicked panic="panicked at crates/node/src/tests/log.rs:"#;
    let panicked = lines[3].strip_prefix(at).expect("the panic line");
    assert!(
        panicked.ends_with(r#":\nthe test panics" mesh_time=none"#),
        "{stderr}"
    );
    assert_eq!(lines.len(), 4, "{stderr}");
    assert!(hook.contains("panicked at"), "{stderr}");
}

#[test]
fn a_panic_with_a_stuck_writer_ends_after_the_bound_with_no_line() {
    let name = "tests::log::a_panic_writes_the_queued_lines_then_the_panic";
    let stderr = child(name, "stuck");
    assert!(!stderr.contains("the test panics"), "{stderr}");
}

#[test]
fn a_panic_in_an_event_after_the_shards_end_reaches_stderr_with_no_abort() {
    let name = "tests::log::a_panic_writes_the_queued_lines_then_the_panic";
    let stderr = child(name, "late");
    assert!(stderr.contains("the field panics"), "{stderr}");
}

#[test]
fn a_panic_of_a_write_waits_for_no_line_and_runs_the_hook_before() {
    let name = "tests::log::a_panic_writes_the_queued_lines_then_the_panic";
    let stderr = child(name, "writes");
    assert!(stderr.contains("the writer panics"), "{stderr}");
    assert!(stderr.contains("the test panics"), "{stderr}");
}

/// The line of an event with `field` and other fields.
fn line(json: bool, field: &str) -> String {
    written(json, || {
        tracing::info!(
            target: "test",
            field,
            count = 3_u64,
            delta = -2_i64,
            ok = true,
            "a\nb"
        );
    })
}

#[test]
fn a_newline_and_a_backslash_in_a_field_stay_on_one_line() {
    assert_eq!(
        line(false, "x\ny\\z\"q"),
        concat!(
            r#"2026-01-01T00:00:00.000000Z INFO test: a\nb "#,
            r#"field="x\ny\\z\"q" count=3 delta=-2 ok=true mesh_time=none"#,
            "\n",
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
