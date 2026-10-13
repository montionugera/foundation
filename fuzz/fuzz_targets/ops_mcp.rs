//! `foundation mcp` never panics or fails, and it writes at most one reply for each
//! line it reads.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use std::pin::pin;
use std::task::{Context, Poll, Waker};

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let cli = ops::cli(
        ["foundation", "mcp"].map(Into::into),
        input,
        &mut output,
        &mut errors,
    );
    // `mcp` reaches no node, so one poll ends it.
    let Poll::Ready(status) = pin!(cli).poll(&mut Context::from_waker(Waker::noop()))
    else {
        panic!("`mcp` waited");
    };
    assert_eq!(
        (status, errors.as_slice()),
        (ops::Run::Exit(0), b"".as_slice())
    );
    let lines = input.split(|&byte| byte == b'\n').count();
    assert!(output.iter().filter(|&&byte| byte == b'\n').count() <= lines);
});
