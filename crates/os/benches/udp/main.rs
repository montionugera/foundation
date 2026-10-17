//! The cost of UDP over the loopback through `os::net`: one datagram against a plain
//! Tokio socket, and a batch sent in one call against the same datagrams sent one by
//! one, also on a socket that cannot use GSO and with a segment over the path MTU.
//! Each sample sends and then receives all the bytes that arrive, except in `register`.

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2955
mod unix;

fn main() {
    divan::main();
}
