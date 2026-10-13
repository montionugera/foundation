//! `foundation mcp` never panics or fails, and it writes at most one reply for each
//! line it reads.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use std::path::Path;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut sim = sim::Sim::new(sim::Config::default());
    let paths = sim.node(sim::node::Config::default()).paths();
    let request = async |_: &[u8]| -> Result<Vec<u8>, ops::Failure> {
        panic!("`mcp` sent a request")
    };
    let reach = async |_: &Path| Ok(request);
    let cli = ops::cli(
        ["foundation", "mcp"].map(Into::into),
        input,
        &mut output,
        &mut errors,
        &paths,
        reach,
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
