//! The cost of writes over the loopback, through `os::net` and through a plain Tokio
//! stream with the same options: one 64-byte round trip, one write of 1 MiB, and 256
//! writes of 64 bytes back to back. The last two reach the unsent bound.

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2955
mod unix;

fn main() {
    divan::main();
}
