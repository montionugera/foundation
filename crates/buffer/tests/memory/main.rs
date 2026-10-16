//! A trim gives back the room of the runs that it drops, so the logs keep the
//! memory bound of S4. This binary has no test harness: the count covers each
//! thread, and a harness allocates on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The bytes of one run.
const RUN: usize = 24;
/// The records of a lap of `buffer::bench::Logs`, one per commit.
const LAP: u32 = 256;
/// The commit of [`wait`] whose trims end the lap that starts at 256.
const LAP_END: u32 = 512;

/// A read that holds 16 records keeps 17 runs of one path, in room for 32. Then the
/// read holds `held` records. Gives the bytes that the trims of the next two laps
/// give back, and checks that the trims of the lap after them give back none.
fn freed(held: u64) -> usize {
    let mut logs = buffer::bench::Logs::new(1);
    for _ in 0..40 {
        logs.trim(1, 16);
    }
    let before = ALLOCATOR.held();
    for _ in 0..2 * LAP {
        logs.trim(1, held);
    }
    let after = ALLOCATOR.held();
    for _ in 0..LAP {
        logs.trim(1, held);
    }
    assert_eq!(
        ALLOCATOR.held(),
        after,
        "a lap that keeps the count of runs gave back bytes"
    );
    before - after
}

/// A read that holds 16 records keeps 17 runs of one path, in room for 32, for 300
/// commits. Then no read holds a record. Gives the number of the commit, from the
/// first, whose trim gives back bytes: the first trim at least one lap after the
/// start of its lap.
fn first_shrink() -> u32 {
    let mut logs = buffer::bench::Logs::new(1);
    for _ in 0..300 {
        logs.trim(1, 16);
    }
    shrink(&mut logs, 300)
}

/// As [`first_shrink`], then a read holds the next 16 records, and then none. Gives
/// the number of the commit whose trim next gives back bytes. The lap that the first
/// shrink starts ends with no shrink, and the lap after it shrinks. So it fails when
/// the first shrink starts its lap at the tail of the hide, not at the trim.
fn next_shrink() -> u32 {
    let mut logs = buffer::bench::Logs::new(1);
    for _ in 0..300 {
        logs.trim(1, 16);
    }
    let commit = shrink(&mut logs, 300);
    for held in 1..=16 {
        logs.trim(1, held);
    }
    shrink(&mut logs, commit + 16)
}

/// Commits with no record held, from the one after `commit`. Gives the number of the
/// first whose trim gives back bytes.
fn shrink(logs: &mut buffer::bench::Logs, mut commit: u32) -> u32 {
    loop {
        let before = ALLOCATOR.held();
        logs.trim(1, 0);
        commit += 1;
        if ALLOCATOR.held() < before {
            return commit;
        }
    }
}

/// S4: each path keeps at most 422 bytes, and room for four paths when there are fewer,
/// of which 8 bytes are the room of its place in the queue of laps that wait, taken
/// when the path is added. [`wait`] checks the bound in a wait. A carry record takes
/// the tail of each of `paths` paths, and a trim to it leaves each with no run. The
/// holder items of the two records, 4 bytes each in a deque that doubles, are a term of
/// S4 per record and path. At 3585 paths the map of places is at its least load, and at
/// 4097 the vector of logs.
fn per_path(paths: u32) {
    let before = ALLOCATOR.held();
    let mut logs = buffer::bench::Logs::new(paths);
    let base = ALLOCATOR.held() - before;
    logs.carry();
    logs.commit(0);
    logs.commit(0);
    let held = ALLOCATOR.held() - before - base;
    let paths = usize::try_from(paths).expect("fits");
    let bound = 422 * paths.max(4) + 16 * paths;
    assert!(
        held <= bound,
        "{held} bytes for {paths} paths, over {bound}"
    );
    drop(logs);
}

/// A read that holds 16 records keeps 17 runs of each of 100,000 paths, in room for 32.
/// Then no read holds a record. Each commit trims 100,000 runs, so the commit at the
/// end of the next lap shrinks the room of 25,256 paths to 4, and the others wait in
/// the queue, in room that it took when each path was added. Gives the bytes that each
/// of that commit and the next 3 takes, and checks the S4 bound of each path in the
/// wait: the bytes held after `Logs::new` but the room of the runs. Then a read holds
/// the next 16 records, and checks that a path that waited shrinks at a later lap end.
fn wait() -> Vec<isize> {
    let paths = 100_000;
    let mut logs = buffer::bench::Logs::new(paths);
    let start = ALLOCATOR.held();
    for _ in 0..40 {
        logs.trim(1, 16);
    }
    for _ in 40..LAP_END {
        logs.trim(1, 0);
    }
    let mut taken = Vec::new();
    for commit in 0..4 {
        let before = ALLOCATOR.held();
        logs.trim(1, 0);
        taken.push(ALLOCATOR.held().cast_signed() - before.cast_signed());
        if commit == 0 {
            let runs = (25_256 * 4 + (100_000 - 25_256) * 32) * RUN;
            let held = ALLOCATOR.held() - start - runs;
            let paths = usize::try_from(paths).expect("fits");
            let bound = 422 * paths + 16 * paths;
            assert!(held <= bound, "{held} bytes in the wait, over {bound}");
        }
    }
    let drained = ALLOCATOR.held();
    for held in 1..=16 {
        logs.trim(1, held);
    }
    for _ in 0..2 * LAP {
        logs.trim(1, 0);
        if ALLOCATOR.held() == drained {
            break;
        }
    }
    assert_eq!(
        ALLOCATOR.held(),
        drained,
        "each path that waited shrinks at a later lap end"
    );
    drop(logs);
    taken
}

fn main() {
    assert_eq!(freed(6), 0, "a lap of at most 8 runs keeps room for 32");
    assert_eq!(
        freed(4),
        (32 - 12) * RUN,
        "a lap of at most 6 runs keeps room for 12"
    );
    assert_eq!(
        freed(2),
        (32 - 8) * RUN,
        "a lap of at most 4 runs keeps room for 8"
    );
    assert_eq!(
        freed(0),
        (32 - 4) * RUN,
        "a lap of at most 2 runs keeps room for 4"
    );
    assert_eq!(
        first_shrink(),
        769,
        "the lap that starts at 512 ends at 768"
    );
    assert_eq!(
        next_shrink(),
        1281,
        "the lap that starts at 768 ends at 1024, and the next at 1280"
    );
    let shrunk = |paths: isize| -> isize { -paths * (32 - 4) * RUN.cast_signed() };
    assert_eq!(
        wait(),
        [
            shrunk(25_256),
            shrunk(25_256),
            shrunk(25_256),
            shrunk(100_000 - 3 * 25_256),
        ],
        "each commit shrinks 256 paths and one for each 4 trims, and allocates no \
         room for the queue"
    );
    for paths in [1, 2, 3, 4, 5, 3585, 4097] {
        per_path(paths);
    }
}
