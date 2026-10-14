//! A trim gives back the room of the runs that it drops, so the logs keep the
//! memory bound of S4. This binary has no test harness: the count covers each
//! thread, and a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The bytes of one run.
const RUN: usize = 24;

/// A read that holds 16 records keeps 17 runs of one path, in room for 32. Then the
/// read holds `held` records, and the next trim drops the runs before them. Gives the
/// bytes that this trim gives back, and checks that a trim after it, which keeps the
/// count of runs, gives back none.
fn freed(held: u64) -> usize {
    let mut logs = buffer::bench::Logs::new(1);
    for _ in 0..40 {
        logs.trim(1, 16);
    }
    let before = ALLOCATOR.held();
    logs.trim(1, held);
    let after = ALLOCATOR.held();
    logs.trim(1, held);
    assert_eq!(
        ALLOCATOR.held(),
        after,
        "a trim that keeps the count of runs gave back bytes"
    );
    before - after
}

fn main() {
    assert_eq!(freed(4), (32 - 14) * RUN, "5 runs keep room for 14");
    assert_eq!(freed(2), (32 - 6) * RUN, "3 runs keep room for 6");
    assert_eq!(freed(0), (32 - 2) * RUN, "1 run keeps room for 2");
}
