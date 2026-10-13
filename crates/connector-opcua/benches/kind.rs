//! The ns in the polls of the run of the `opcua` kind for one tick, against the sim
//! server over the sim net: the read of 1 or of 256 nodes, its frame, and the write of
//! the frame, with no time of the server. Run with `cargo bench -p connector-opcua
//! --bench kind`.

#![expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]

use std::time::Instant;

#[path = "../tests/common/kind.rs"]
mod kind;

/// The node counts of the reads.
const COUNTS: [usize; 2] = [1, 256];
/// The ticks before the timed ticks: tick 1 connects.
const WARMUP: usize = 10;
const TICKS: usize = 1000;

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    println!("ns per tick over {TICKS} ticks");
    println!("{:<10} {:>7} {:>7} {:>7}", "nodes", "p10", "p50", "p90");
    for nodes in COUNTS {
        let mut ticks = kind::ticks(nodes, WARMUP + TICKS, |poll| {
            let start = Instant::now();
            poll();
            let nanos = Instant::now().duration_since(start).as_nanos();
            u64::try_from(nanos).expect("a poll takes under 2^64 ns")
        });
        let mut ticks = ticks.split_off(WARMUP);
        ticks.sort_unstable();
        let at = |percent: usize| ticks[ticks.len() * percent / 100];
        println!("{nodes:<10} {:>7} {:>7} {:>7}", at(10), at(50), at(90));
    }
}
