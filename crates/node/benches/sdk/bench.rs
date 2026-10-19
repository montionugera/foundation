//! The rounds and the table of the bench.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use env::tasks::{Group, Tasks};
use program::write::{Config, Outcome, Time};
use sdk::Client;
use types::authority::Authority;
use types::frame::Form;
use types::name::Name;
use types::sample::Value;

/// Calls before the timed rounds.
const WARMUP: usize = 500;

/// The spin of the caller before each call, which is longer than the time for the
/// task to park.
const GAP: Duration = Duration::from_micros(100);

/// Runs the bench and prints its table.
pub(crate) fn run() {
    let (rounds, calls) = (5, 1_000);
    let legs = legs(rounds, calls);
    let (mut rig, node, subject) = crate::plant::planted();
    let program = program(&node, &subject, rounds, calls);
    let client = Client::connect(&node, subject).expect("the client connects");
    let config = sdk::write::Config::new(names());
    let mut writer = client.open_writer(config).expect("the writer opens");
    let values = [(0, Value::I64(1)), (1, Value::I64(2))];
    let sdk = rounded(rounds, calls, || {
        spin();
        let start = Instant::now();
        let written = writer.write(&values);
        let span = start.elapsed();
        assert!(
            matches!(written, Ok(sdk::write::Outcome::Taken { .. })),
            "{written:?}"
        );
        span
    });
    assert_eq!(writer.close(), Ok(()), "the writer closes");
    assert_eq!(client.close(), Ok(()), "the client closes");
    rig.stop();
    println!(
        "{:<14} {:>6} {:>9} {:>9}",
        "line", "round", "p50 µs", "p99 µs"
    );
    for (name, spans) in [
        ("leg to task", legs.0),
        ("leg to caller", legs.1),
        ("sdk", sdk),
        ("program", program),
    ] {
        for (round, mut spans) in spans.into_iter().enumerate() {
            spans.sort_unstable();
            let at = |percent: usize| micros(spans[spans.len() * percent / 100]);
            println!("{name:<14} {round:>6} {:>9.1} {:>9.1}", at(50), at(99));
        }
    }
}

/// The spans of each of `rounds` rounds of `calls` calls of `call`, after
/// [`WARMUP`] calls.
fn rounded(
    rounds: usize,
    calls: usize,
    mut call: impl FnMut() -> Duration,
) -> Vec<Vec<Duration>> {
    for _ in 0..WARMUP {
        call();
    }
    let round = |_| (0..calls).map(|_| call()).collect();
    (0..rounds).map(round).collect()
}

/// The spans of each leg between this thread and a task on an `os` shard, in
/// `rounds` rounds of `calls` round trips.
fn legs(rounds: usize, calls: usize) -> (Vec<Vec<Duration>>, Vec<Vec<Duration>>) {
    let (to_task, mut taken) = tokio::sync::mpsc::channel::<Instant>(1);
    let (to_caller, answers) = mpsc::sync_channel::<(Duration, Instant)>(1);
    let main = move |tasks: Tasks| async move {
        let group = Group::new(tasks);
        while let Some(sent) = taken.recv().await {
            let span = sent.elapsed();
            to_caller
                .try_send((span, Instant::now()))
                .expect("the caller waits for each answer");
        }
        group.ended().await;
    };
    let handle = started("task", main);
    let mut back = Vec::new();
    let mut trip = || {
        spin();
        to_task
            .try_send(Instant::now())
            .expect("the task took the call before");
        let (span, sent) = answers.recv().expect("the task answers");
        back.push(sent.elapsed());
        span
    };
    let forth = rounded(rounds, calls, &mut trip);
    drop(to_task);
    handle.join().expect("the task ends");
    let back = back[WARMUP..].chunks(calls).map(<[_]>::to_vec).collect();
    (forth, back)
}

/// The spans of each of `rounds` rounds of `calls` writes of 2 values through a
/// `program::Writer` at `node`, on an `os` shard of its own.
fn program(
    node: &sdk::Node,
    subject: &sdk::Subject,
    rounds: usize,
    calls: usize,
) -> Vec<Vec<Duration>> {
    let (node, subject) = (node.clone(), subject.clone());
    let (sender, spans) = mpsc::channel();
    let main = move |tasks: Tasks| async move {
        let group = Group::new(tasks);
        let session = crate::plant::session(node, subject, group.tasks().clone()).await;
        let config = Config {
            authority: Authority(0),
            lease: None,
            channels: names(),
        };
        let mut writer = session.writer(config).await.expect("the writer opens");
        let index = writer.index();
        let layout: Vec<_> = (0..writer.places().len()).map(|p| (p, 8)).collect();
        let mut all = Vec::with_capacity(WARMUP + rounds * calls);
        for _ in 0..WARMUP + rounds * calls {
            let start = Instant::now();
            let draft = writer
                .draft(Form::Raw, 1, &layout)
                .await
                .expect("the frame fits the pool");
            for (place, _) in layout.iter().filter(|(place, _)| *place != index) {
                draft
                    .series_mut(*place)
                    .expect("the layout holds it")
                    .fill(1);
            }
            let written = writer.write(Time::Node).await;
            all.push(start.elapsed());
            assert!(matches!(written, Ok(Outcome::Taken { .. })), "{written:?}");
        }
        writer.finish().await.expect("the writer finishes");
        drop(session);
        group.ended().await;
        sender.send(all).expect("the bench waits for the spans");
    };
    let handle = started("program", main);
    let all = spans.recv().expect("the program thread sends its spans");
    handle.join().expect("the program thread ends");
    all[WARMUP..].chunks(calls).map(<[_]>::to_vec).collect()
}

/// The channels that each writer writes.
fn names() -> Vec<Name> {
    ["plant.a", "plant.b"]
        .map(|name| name.parse().expect("a name"))
        .into()
}

/// The thread `name` on a shard of `os`, which runs `main`.
fn started<M, F>(name: &str, main: M) -> env::thread::Handle
where
    M: FnOnce(Tasks) -> F + Send + 'static,
    F: Future<Output = ()> + 'static,
{
    let config = env::shards::Config {
        name: name.into(),
        core: None,
    };
    let log = tracing::dispatcher::get_default(Clone::clone);
    let shards = os::shards().expect("the shards start").logged(log);
    shards.start(config, main).expect("the thread starts")
}

/// Spins for [`GAP`].
fn spin() {
    let start = Instant::now();
    while start.elapsed() < GAP {
        std::hint::spin_loop();
    }
}

fn micros(span: Duration) -> f64 {
    span.as_secs_f64() * 1e6
}
