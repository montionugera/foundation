//! The time for each commit of the write-ahead ring with no file, and its records per
//! second: `append` and `synced` for each record, `trimmed` and `release` for each
//! commit. A commit of 1 record shows the cost for each commit, one of 8 the cost for
//! each record. The ring of ID 64 wraps often, the one of ID 4096 seldom. Each time
//! also holds the bench's own work to make the ends of each record, so a change of the
//! writer shows smaller than it is. Each ring has [`PHASE`] blocks more than its
//! argument. A commit trims only once the tail is behind the low mark, so each sample
//! holds [`SAMPLE_SIZE`] commits and at least 10 trims. `commit_group` times one group
//! commit of 1 or 32 entries on the ring of ID 64: the fit, which checks that the
//! record fits, the take, the close, the seal with the CRC of the body, and the same
//! trim and sync. `queue_group` adds the append of each entry to the logs, as the
//! buffer queues a batch.

use buffer::bench::{Group, PHASE, Ring, SAMPLE_SIZE};
use divan::Bencher;
use divan::counter::ItemsCount;

fn main() {
    divan::main();
}

#[divan::bench(args = [64, 4096], sample_size = SAMPLE_SIZE)]
fn commit_1_record(bencher: Bencher<'_, '_>, blocks: u64) {
    cycle(bencher, blocks, 1);
}

#[divan::bench(args = [64, 4096], sample_size = SAMPLE_SIZE)]
fn commit_8_records(bencher: Bencher<'_, '_>, blocks: u64) {
    cycle(bencher, blocks, 8);
}

fn cycle(bencher: Bencher<'_, '_>, blocks: u64, records: usize) {
    let mut ring = steady_ring(blocks, records);
    bencher
        .counter(ItemsCount::new(records))
        .bench_local(|| ring.commit(divan::black_box(records)));
}

/// The ring of `blocks`, after the warm-up, so a time holds the steady ring, which
/// trims in steps, not one still filling. Out of line, so that a change of the
/// setup does not change the code of the timed loop.
#[inline(never)]
fn steady_ring(blocks: u64, records: usize) -> Ring {
    let mut ring = Ring::new(blocks + PHASE);
    for _ in 0..2 * blocks {
        ring.commit(records);
    }
    ring
}

#[divan::bench(args = [1, 32], sample_size = SAMPLE_SIZE)]
fn commit_group(bencher: Bencher<'_, '_>, entries: usize) {
    let mut group = steady_group(entries, Group::commit);
    bencher
        .counter(ItemsCount::new(entries))
        .bench_local(|| group.commit(divan::black_box(entries)));
}

#[divan::bench(args = [1, 32], sample_size = SAMPLE_SIZE)]
fn queue_group(bencher: Bencher<'_, '_>, entries: usize) {
    let mut group = steady_group(entries, Group::queue);
    bencher
        .counter(ItemsCount::new(entries))
        .bench_local(|| group.queue(divan::black_box(entries)));
}

/// The group ring of 64 blocks after the warm-up of `commit`, out of line as
/// [`steady_ring`] is.
#[inline(never)]
fn steady_group(entries: usize, commit: fn(&mut Group, usize)) -> Group {
    let mut group = Group::new(64 + PHASE);
    for _ in 0..2 * 64 {
        commit(&mut group, entries);
    }
    group
}
