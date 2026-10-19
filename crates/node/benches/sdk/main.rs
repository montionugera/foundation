//! The time of a write of 2 values through `sdk` and through `program`, against a node
//! process on loopback, and of the two legs that `sdk` adds between the caller and the
//! client's thread. Run with `cargo bench -p node --bench sdk`.
//!
//! Each line gives the p50 and p99 of the calls of each round, in µs:
//!
//! - `leg to task`: from a `try_send` on a Tokio channel of 1 to the return of the
//!   `recv` of a task on an `os` shard, as `Writer::send` sends each call to the task
//!   of `served` in `sdk/rust/src/write.rs`.
//! - `leg to caller`: from the task's `try_send` on a `std` `sync_channel` of 1 to the
//!   return of the caller's `recv`, as `served` sends each outcome to `wait`.
//! - `sdk`: `sdk::Writer::write` of 2 values with the node's stamp.
//! - `program`: `program::Writer::draft` and `write` of the same frame, on an `os`
//!   shard of its own, as `served` writes it.
//!
//! The channels are those of `Writer::open`, and each shard is one of `Client::start`
//! in `sdk/rust/src/lib.rs`: `os`, with the caller's log. Before each call of the
//! first three lines, the caller spins for [`GAP`], so that the task parks before each
//! send, as it does between the writes of a program.
//!
//! Each write must give `Taken`: the ring of the home shard holds 17,000 to 20,000
//! frames of the bench, and a frame past it is `Lost`. The `sdk` and `program` lines
//! write 11,000 together, also under `cargo test`, so that a test fails when they do
//! not fit. Judge `sdk` against `program` from one run, and each leg against its p50: a
//! p99 far over it is a wait for a wake.
//!
//! Only Unix: the rig stops its node with SIGTERM.

/// Only Unix: the rounds and the table of the bench.
#[cfg(all(unix, not(loom)))]
#[expect(clippy::print_stdout, reason = "a bench prints its table")]
#[expect(
    clippy::disallowed_methods,
    reason = "a benchmark reads a real clock and blocks on its threads"
)]
mod bench;
/// Only Unix: the channels of the bench, and the session of its `program` line.
#[cfg(all(unix, not(loom)))]
#[expect(
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a helper of the node tests fails loud"
)]
#[path = "../../tests/it/plant.rs"]
mod plant;
/// Only Unix: the helpers of the node tests that start and stop a node.
#[cfg(all(unix, not(loom)))]
#[expect(
    dead_code,
    clippy::missing_assert_message,
    reason = "the bench uses only some of the helpers of the node tests"
)]
#[path = "../../tests/it/rig.rs"]
mod rig;
/// Only Unix: the output of `foundation status --json`, which the rig reads.
#[cfg(all(unix, not(loom)))]
#[expect(dead_code, reason = "the bench uses only some of the helpers")]
#[path = "../../tests/it/status.rs"]
mod status;

/// Only Unix: runs the bench.
#[cfg(all(unix, not(loom)))]
fn main() {
    bench::run();
}

#[cfg(not(all(unix, not(loom))))]
fn main() {}

/// Only Unix: `bytes` as UTF-8, for the helpers of the node tests.
#[cfg(all(unix, not(loom)))]
fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8")
}
