//! The cost of the origin of one request, for each form of host. Run with `cargo
//! bench -p connector --bench origin`.
//!
//! - Each host line: `bench::origin` of a URI, with the drop of the origin, as each
//!   request of `Client::send` makes it.
//! - `uri`: the parse of the URI text by `http`, code that `connector` does not
//!   change, to show the noise of the machine.
//! - `cmp`: one compare of two origins, as the pool of connections makes it.
//!
//! Each figure is ps per call, with the count of the allocations of the calls.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::hint::black_box;
use std::time::Instant;

use connector::http::bench;
use http::Uri;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const CALLS: u64 = 10_000;
const WARMUP: usize = 20;
const ROUNDS: usize = 200;

const HOSTS: [(&str, &str); 5] = [
    (
        "name",
        "http://influx:8086/api/v2/write?org=plant&bucket=edge",
    ),
    (
        "long",
        "http://historian.plant-a.example.com:8086/api/v2/write",
    ),
    ("ipv4", "http://10.0.0.2:8086/api/v2/write"),
    ("ipv6", "http://[fd00::2]:8086/api/v2/write"),
    ("zone", "http://[fe80::1%252]:8086/api/v2/write"),
];

fn main() {
    let mut lines = Vec::new();
    for (name, text) in HOSTS {
        let uri = parse(text);
        lines.push(time(name, || {
            black_box(bench::origin(black_box(&uri)).expect("a valid URI"));
        }));
    }
    let text = HOSTS[0].1;
    lines.push(time("uri", || {
        black_box(black_box(text).parse::<Uri>().expect("a valid URI"));
    }));
    let a = bench::origin(&parse(HOSTS[0].1)).expect("a valid URI");
    let b = bench::origin(&parse("http://influx-b:8086/")).expect("a valid URI");
    lines.push(time("cmp", || {
        black_box(black_box(&a).cmp(black_box(&b)));
    }));
    print(&lines);
}

fn parse(text: &str) -> Uri {
    text.parse().expect("a valid URI")
}

/// The ps per call of each timed round, and the allocations of all of them.
struct Line {
    name: &'static str,
    picos: Vec<u64>,
    allocations: u64,
}

/// Times `ROUNDS` rounds of `CALLS` calls of `call`, after `WARMUP` rounds.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn time(name: &'static str, mut call: impl FnMut()) -> Line {
    let mut line = Line {
        name,
        picos: Vec::new(),
        allocations: 0,
    };
    for round in 0..WARMUP + ROUNDS {
        let start = Instant::now();
        let ((), allocations) = ALLOCATOR.count(|| {
            for _ in 0..CALLS {
                call();
            }
        });
        let nanos = Instant::now().duration_since(start).as_nanos();
        if round >= WARMUP {
            let picos =
                u64::try_from(nanos * 1000).expect("a round takes under 2^64 ps");
            line.picos.push(picos / CALLS);
            line.allocations += allocations;
        }
    }
    line
}

/// Prints p10, p50, and p90 of the ps per call of each line, and its allocations.
#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn print(lines: &[Line]) {
    println!("ps per call over {ROUNDS} rounds of {CALLS} calls");
    println!(
        "{:<10} {:>7} {:>7} {:>7} {:>7}",
        "line", "p10", "p50", "p90", "allocs"
    );
    for line in lines {
        let mut picos = line.picos.clone();
        picos.sort_unstable();
        let at = |percent: usize| picos[picos.len() * percent / 100];
        println!(
            "{:<10} {:>7} {:>7} {:>7} {:>7}",
            line.name,
            at(10),
            at(50),
            at(90),
            line.allocations
        );
    }
}
