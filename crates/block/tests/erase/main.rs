//! A pool on `Erased` leaves the bytes of its blocks in no freed allocation. The
//! scan covers every thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use block::{Config, Erased, Heap, Memory, Pool};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const PLAIN: [u8; 32] = [0x5a; 32];

fn main() {
    // Miri stops at the read of a freed byte that the program never wrote (COUNTING
    // ALLOCATOR). The unit tests of `Erased` run its erase under Miri.
    if cfg!(miri) {
        return;
    }
    for (len, at) in [(32, 0), (3000, 1), (3000, 0)] {
        let held = freed(Heap::new, |pool| lent(pool, len, at));
        assert_eq!(
            held, 1,
            "a dropped `Heap::new` frees the bytes ({len}, {at})"
        );
        let held = freed(Erased::new, |pool| lent(pool, len, at));
        assert_eq!(held, 0, "a dropped `Erased` erases them ({len}, {at})");
    }
    assert_eq!(freed(Heap::new, recarved), 1, "`Heap::new` frees the bytes");
    assert_eq!(
        freed(Erased::new, recarved),
        0,
        "a dropped `Erased` erases a block that the pool carved again after a purge"
    );
}

/// Gives the count of freed allocations that hold `PLAIN`, after `case` runs on a pool
/// on the memory that `memory` gives and drops it.
fn freed<M: Memory + 'static>(memory: fn(usize) -> M, case: impl FnOnce(Pool)) -> u64 {
    let config = Config::new(1 << 14).expect("the budget fits");
    let memory = memory(config.reservation());
    let ((), found) =
        ALLOCATOR.freed_holding(&PLAIN, || case(Pool::new(config, memory)));
    found
}

/// Drops `pool` while it lends a block of 32 bytes and one of `len` bytes, then the
/// blocks. Block `at` holds `PLAIN` at its end.
fn lent(pool: Pool, len: usize, at: usize) {
    let blocks: Vec<_> = [32, len]
        .into_iter()
        .enumerate()
        .map(|(index, len)| {
            let mut block = pool.alloc(len).expect("the budget has room");
            block.fill(0x11);
            if index == at {
                block[len - PLAIN.len()..].copy_from_slice(&PLAIN);
            }
            block.freeze()
        })
        .collect();
    drop(pool);
    assert!(
        blocks.iter().any(|block| block.ends_with(&PLAIN)),
        "a lent block outlives its pool"
    );
}

/// Puts `PLAIN` in a block, purges twice, so the idle class gives its pages back, and
/// carves the class again with a block that ends before `PLAIN`.
fn recarved(pool: Pool) {
    let mut first = pool.alloc(32).expect("the budget has room");
    first.fill(0x11);
    let mut second = pool.alloc(32).expect("the budget has room");
    second.copy_from_slice(&PLAIN);
    drop(first);
    drop(second);
    pool.purge();
    pool.purge();
    drop(pool.alloc(32).expect("the budget has room"));
    drop(pool);
}
