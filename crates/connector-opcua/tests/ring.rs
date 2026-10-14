//! A connector of the `opcua` kind whose writes fill the ring of its home, which the
//! test home never trims (#160), ends its call with `Ok` in bounded sim steps.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "common/kind.rs"]
#[expect(dead_code, reason = "this binary runs only the writes")]
mod kind;

use types::sample::Scalar;

#[test]
fn a_run_that_writes_past_the_ring_of_its_home_returns_ok() {
    let writes = kind::writes(256, &[Scalar::F64], 8_000, |work| {
        work();
        0
    });
    assert_eq!(writes.len(), 8_000, "each write of the run");
}
