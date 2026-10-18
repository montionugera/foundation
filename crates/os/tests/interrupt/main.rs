//! `os::interrupt` holds the first SIGINT or SIGTERM and completes its future at it,
//! and a second one ends the process. This binary has no test harness: the threads
//! of a harness start before the hold, so a signal would end the process. With the
//! argument `child`, it is the process that the test signals, with `blocked`, one
//! that blocks SIGTERM first, with `pending`, one that also makes it pending, and with
//! `ignored`, one that ignores SIGINT and sends it to itself. With `both`, it blocks
//! both and writes whether the future stays pending.

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2957
mod unix;

fn main() {
    #[cfg(any(target_os = "linux", target_os = "macos"))] // #2957
    unix::main();
}
