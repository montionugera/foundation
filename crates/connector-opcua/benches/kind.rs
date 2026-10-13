//! The ns of the `opcua` kind against the sim server over the sim net: the polls of
//! its run for one tick (the read of 1 or of 256 nodes, its frame, and the write of
//! the frame, with no time of the server), the check of the values of one read and
//! the write of its frame alone, the polls of its run start, and its parse. Run with
//! `cargo bench -p connector-opcua --bench kind`.

#![expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]

use std::time::Instant;

use connector::kind::Kind as _;

#[path = "../tests/common/kind.rs"]
mod kind;

/// The node counts of the reads of a tick.
const COUNTS: [usize; 2] = [1, 256];
/// The ticks or writes before the timed ones: tick 1 connects, and the writer of hub
/// allocates in some of the first 19 writes.
const WARMUP: usize = 20;
const TICKS: usize = 1000;
/// The node counts of a run start and of a parse. A frame of 1,000 nodes fits the
/// largest block of the pool of the test hub.
const STARTS: [usize; 2] = [250, 1_000];
const RUNS: usize = 5;
const PARSES: usize = 20;

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    println!("ns per tick over {TICKS} ticks");
    println!("{:<10} {:>7} {:>7} {:>7}", "nodes", "p10", "p50", "p90");
    for nodes in COUNTS {
        let mut ticks = kind::run(nodes, WARMUP + TICKS, time)
            .ticks
            .split_off(WARMUP);
        ticks.sort_unstable();
        let at = |percent: usize| ticks[ticks.len() * percent / 100];
        println!("{nodes:<10} {:>7} {:>7} {:>7}", at(10), at(50), at(90));
    }
    println!("ns per write of a frame over {TICKS} writes");
    println!("{:<10} {:>7} {:>7} {:>7}", "nodes", "p10", "p50", "p90");
    for nodes in COUNTS {
        let mut writes = kind::writes(nodes, WARMUP + TICKS, time).split_off(WARMUP);
        writes.sort_unstable();
        let at = |percent: usize| writes[writes.len() * percent / 100];
        println!("{nodes:<10} {:>7} {:>7} {:>7}", at(10), at(50), at(90));
    }
    println!("ns per run start over {RUNS} runs, and per parse over {PARSES} parses");
    println!("{:<10} {:>10} {:>10}", "nodes", "start p50", "parse p50");
    for nodes in STARTS {
        let starts = (0..RUNS).map(|_| kind::run(nodes, 1, time).start);
        let config = kind::config("opc.tcp://plc:4840", nodes);
        let parses = (0..PARSES).map(|_| {
            let mut parsed = None;
            let nanos = time(&mut || {
                parsed = Some(connector_opcua::Kind::default().parse(&config));
            });
            let errors = parsed.and_then(Result::err);
            assert!(
                errors.is_none(),
                "the config of {nodes} reads parses: {errors:?}"
            );
            nanos
        });
        let (start, parse) = (median(starts.collect()), median(parses.collect()));
        println!("{nodes:<10} {start:>10} {parse:>10}");
    }
}

/// The ns of `work`.
fn time(work: &mut dyn FnMut()) -> u64 {
    let start = Instant::now();
    work();
    let nanos = Instant::now().duration_since(start).as_nanos();
    u64::try_from(nanos).expect("the work takes under 2^64 ns")
}

fn median(mut values: Vec<u64>) -> u64 {
    values.sort_unstable();
    values[values.len() / 2]
}
