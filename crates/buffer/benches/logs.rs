//! The time to sync the durable logs of a shard at each commit, for each entry: one
//! record with data entries of each index, after a trim that hides each older record.
//! Also the time to append an entry, to carry a tail, the time of a commit that
//! trims to the oldest holder, the time of a commit whose carry fills past its
//! reach, the time of a commit that syncs entries with no samples, and the time of
//! the commits that end a lap of each index.

use buffer::bench::Logs;
use divan::Bencher;
use divan::counter::ItemsCount;

fn main() {
    divan::main();
}

/// Times logs that hold their runs, not logs still filling.
fn warm(indexes: u32) -> Logs {
    let mut logs = Logs::new(indexes);
    for _ in 0..16 {
        logs.commit(1);
    }
    logs
}

fn count(indexes: u32) -> ItemsCount {
    ItemsCount::new(usize::try_from(indexes).expect("u32 fits usize"))
}

/// Each entry starts a run in its record.
#[divan::bench(args = [1, 64, 256])]
fn sync(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = warm(indexes);
    bencher
        .counter(count(indexes))
        .bench_local(|| logs.commit(1));
}

/// Four entries of each index in each record, so three of four start no run.
#[divan::bench(args = [1, 64, 256])]
fn sync_four(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = warm(indexes);
    bencher
        .counter(count(indexes * 4))
        .bench_local(|| logs.commit(4));
}

#[divan::bench(args = [1, 64, 256])]
fn append(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = warm(indexes);
    bencher
        .counter(count(indexes))
        .bench_local(|| logs.append());
}

/// A commit with an append and a sync of each entry, then a carry record of the
/// tail of each index. Less `append` and `sync`, the time to carry each tail.
#[divan::bench(args = [1, 64, 256])]
fn carry(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = warm(indexes);
    bencher.counter(count(indexes)).bench_local(|| logs.carry());
}

/// One index syncs each 64th commit, so the records after its holder stay.
#[divan::bench(args = [64, 256])]
fn trim(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = trimmed(indexes, 0);
    bencher
        .counter(count(indexes))
        .bench_local(|| logs.trim(64, 0));
}

/// As `trim`, while a read holds the record 64 commits back, so no trim passes the
/// items before the oldest holder.
#[divan::bench(args = [64, 256])]
fn trim_held(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = trimmed(indexes, 64);
    bencher
        .counter(count(indexes))
        .bench_local(|| logs.trim(64, 64));
}

/// One index syncs each 128th commit, and the carry reaches 64 records back. The
/// time of the commit whose carry takes its tail and fills from the stale items of
/// the 64 records after it.
#[divan::bench(args = [1000])]
fn fill(bencher: Bencher<'_, '_>, indexes: u32) {
    bencher
        .counter(count(indexes))
        .with_inputs(|| {
            let mut logs = Logs::new(indexes);
            for _ in 0..65 {
                logs.fill(128, 64);
            }
            logs
        })
        .bench_local_values(|mut logs| {
            logs.fill(128, 64);
            logs
        });
}

/// Each commit syncs an entry with no samples of each index and carries each tail.
/// The trim of the next commit lands at the oldest holder, the carry record.
#[divan::bench(args = [1, 64, 256])]
fn mark(bencher: Bencher<'_, '_>, indexes: u32) {
    let mut logs = Logs::new(indexes);
    for _ in 0..16 {
        logs.mark();
    }
    bencher.counter(count(indexes)).bench_local(|| logs.mark());
}

/// A read holds 16 records for 40 commits, so the room of the runs of each index
/// grows to 32, then no read holds a record. The commit at the end of the next lap
/// shrinks the room of as many indexes as its budget allows, and the next three end
/// the wait of the rest. The time of those four commits.
#[divan::bench(args = [10_000])]
fn lap_end(bencher: Bencher<'_, '_>, indexes: u32) {
    bencher
        .counter(count(4 * indexes))
        .with_inputs(|| {
            let mut logs = Logs::new(indexes);
            for _ in 0..40 {
                logs.trim(1, 16);
            }
            for _ in 40..512 {
                logs.trim(1, 0);
            }
            logs
        })
        .bench_local_values(|mut logs| {
            for _ in 0..4 {
                logs.trim(1, 0);
            }
            logs
        });
}

fn trimmed(indexes: u32, held: u64) -> Logs {
    let mut logs = Logs::new(indexes);
    for _ in 0..128 {
        logs.trim(64, held);
    }
    logs
}
