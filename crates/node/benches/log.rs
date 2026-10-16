//! The cost of one event through the log of a node, before its write. Run with
//! `cargo bench -p node --bench log`.
//!
//! - `started`: the event `started`, with three fields, as text and as JSON.
//! - `stopped`: the event `stopped`, with no field.
//! - `filtered`: a `debug` event under the level `info`, as noq-proto writes for each
//!   packet.
//! - `undispatched`: the control, the same event with no dispatch, as before a node
//!   had a log.
//! - `span`: a `trace` span entered and dropped under the level `trace`.
//! - `build`: the build of the dispatch, once for each node.
//!
//! The writer is `io::sink`, so no line counts its write to stderr.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::io;
use std::net::SocketAddr;

use divan::Bencher;
use tracing::{Dispatch, Level};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    check();
    divan::main();
}

fn dispatch(json: bool, level: Level) -> Dispatch {
    let wall = os::wall().expect("a wall clock");
    let (_mesh, reader) = clock::Clock::new(os::clock());
    node::bench::log(log(json, level), wall, reader)
}

fn log(json: bool, level: Level) -> node::Log {
    node::Log {
        writer: Box::new(io::sink()),
        format: ops::Log { json, level },
    }
}

fn started() {
    let address: SocketAddr = ([127, 0, 0, 1], 9090).into();
    tracing::info!(
        name = %divan::black_box("edge"),
        address = %divan::black_box(address),
        shards = divan::black_box(16_usize),
        "started"
    );
}

fn filtered() {
    tracing::debug!(pn = divan::black_box(7_u64), "packet");
}

/// An event under the level allocates nothing, and one at the level allocates its line.
fn check() {
    let d = dispatch(false, Level::INFO);
    let ((), allocations) =
        tracing::dispatcher::with_default(&d, || ALLOCATOR.count(filtered));
    assert_eq!(allocations, 0, "allocations of a filtered event");
    let ((), allocations) =
        tracing::dispatcher::with_default(&d, || ALLOCATOR.count(started));
    assert_ne!(allocations, 0, "allocations of an event at the level");
}

#[divan::bench(args = [false, true])]
fn started_line(bencher: Bencher<'_, '_>, json: bool) {
    let d = dispatch(json, Level::INFO);
    tracing::dispatcher::with_default(&d, || bencher.bench_local(started));
}

#[divan::bench]
fn stopped(bencher: Bencher<'_, '_>) {
    let d = dispatch(false, Level::INFO);
    tracing::dispatcher::with_default(&d, || {
        bencher.bench_local(|| tracing::info!("stopped"));
    });
}

#[divan::bench]
fn filtered_event(bencher: Bencher<'_, '_>) {
    let d = dispatch(false, Level::INFO);
    tracing::dispatcher::with_default(&d, || bencher.bench_local(filtered));
}

#[divan::bench]
fn span(bencher: Bencher<'_, '_>) {
    let d = dispatch(false, Level::TRACE);
    tracing::dispatcher::with_default(&d, || {
        bencher.bench_local(|| {
            let span = tracing::trace_span!("frame", ty = %divan::black_box("STREAM"));
            drop(span.entered());
        });
    });
}

#[divan::bench]
fn build(bencher: Bencher<'_, '_>) {
    let wall = os::wall().expect("a wall clock");
    let (_mesh, reader) = clock::Clock::new(os::clock());
    bencher
        .with_inputs(|| (wall.clone(), reader.clone()))
        .bench_local_values(|(wall, reader)| {
            node::bench::log(log(false, Level::INFO), wall, reader)
        });
}

#[divan::bench]
fn undispatched(bencher: Bencher<'_, '_>) {
    bencher.bench_local(filtered);
}
