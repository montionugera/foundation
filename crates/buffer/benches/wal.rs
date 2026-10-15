//! The time for each commit of the write-ahead ring with no file, and its records per
//! second: `append` and `synced` for each record, `trimmed` and `release` for each
//! commit. A commit of 1 record shows the cost for each commit, one of 8 the cost for
//! each record. The ring of ID 64 wraps often, the one of ID 4096 seldom. Each time
//! also holds the bench's own work to make the ends of each record, so a change of the
//! writer shows smaller than it is. Each ring has [`PHASE`] blocks more than its
//! argument, so that each commit of 1 record trims.

use buffer::bench::{PHASE, Ring};
use divan::Bencher;
use divan::counter::ItemsCount;

fn main() {
    divan::main();
}

#[divan::bench(args = [64, 4096])]
fn commit_1_record(bencher: Bencher<'_, '_>, blocks: u64) {
    cycle(bencher, blocks, 1);
}

#[divan::bench(args = [64, 4096])]
fn commit_8_records(bencher: Bencher<'_, '_>, blocks: u64) {
    cycle(bencher, blocks, 8);
}

fn cycle(bencher: Bencher<'_, '_>, blocks: u64, records: usize) {
    let mut ring = Ring::new(blocks + PHASE);
    // Times the steady ring, which trims at each commit, not one still filling.
    for _ in 0..2 * blocks {
        ring.commit(records);
    }
    bencher
        .counter(ItemsCount::new(records))
        .bench_local(|| ring.commit(divan::black_box(records)));
}
