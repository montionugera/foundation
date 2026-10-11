//! `node::name` on a data directory with no file `name` and no file `node.key` makes no
//! pool for the read of a founding, whose reservation is over 2 MiB. The count covers
//! each thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The most bytes that the call may hold at once: far under the reservation of the pool
/// of the founding read, and over what the reads of the sector files hold.
const BOUND: usize = 64 << 10;

fn main() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    // The first run makes the allocations that a sim makes once.
    assert_eq!(
        sim.run_on(&host, |_, _| async {}),
        Ok(()),
        "the first run ends"
    );
    let before = ALLOCATOR.held();
    ALLOCATOR.reset_peak();
    let named = sim.run_on(&host, |host, _| async move {
        node::name(&host.files(), None).await
    });
    let peak = ALLOCATOR.peak() - before;
    assert_eq!(
        named,
        Ok(Err(node::Error::Unnamed)),
        "an empty data directory holds no name"
    );
    assert!(peak < BOUND, "the call held {peak} bytes at once");
}
