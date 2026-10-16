//! The cost to parse one `Host` of each form. `connector::http` parses the host of
//! its URI on each request. Run with `cargo bench -p env --bench host`. Before it
//! times anything, it prints the allocations of each parse.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use divan::black_box;
use env::net::host::Host;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const HOSTS: [&str; 4] = [
    "influx",
    "historian.plant-a.example.com",
    "10.0.0.2",
    "[fd00::2]",
];

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    for text in HOSTS {
        let (_host, allocs) = ALLOCATOR.count(|| text.parse::<Host>());
        println!("{text}: {allocs} allocations per parse");
    }
    divan::main();
}

#[divan::bench(args = HOSTS)]
fn parse(text: &str) -> Host {
    black_box(text).parse().expect("a host")
}
