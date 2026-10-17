//! A write and a read of one message over the loopback, and a send and a receive of
//! a UDP batch, make no heap allocation after the first poll of each end, which
//! registers the socket. Nor does a write past the unsent bound, or one that waits
//! for the bound. This binary has no test harness: the count covers each thread, and
//! a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2955
mod unix;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    #[cfg(any(target_os = "linux", target_os = "macos"))] // #2955
    unix::main();
}
