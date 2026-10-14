//! A pool from `Pool::erased` leaves the bytes of its blocks in no freed allocation.
//! The scan covers every thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use block::{Config, Pool};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const PLAIN: [u8; 32] = [0x5a; 32];

fn main() {
    // Miri stops at the read of a freed byte that the program never wrote (COUNTING
    // ALLOCATOR). The unit tests of `block` run the erase under Miri.
    if cfg!(miri) {
        return;
    }
    for (len, at) in [(32, 0), (3000, 1), (3000, 0)] {
        let held = freed(Pool::heap, 1 << 14, |pool| lent(pool, len, at));
        assert_eq!(
            held, 1,
            "a dropped `Pool::heap` frees the bytes ({len}, {at})"
        );
        let held = freed(Pool::erased, 1 << 14, |pool| lent(pool, len, at));
        assert_eq!(
            held, 0,
            "a dropped `Pool::erased` erases them ({len}, {at})"
        );
    }
    assert_eq!(
        freed(Pool::heap, 1 << 14, recarved),
        1,
        "`Pool::heap` frees the bytes"
    );
    assert_eq!(
        freed(Pool::erased, 1 << 14, recarved),
        0,
        "a dropped `Pool::erased` erases a block that it carved again after a purge"
    );
    assert_eq!(
        freed(Pool::heap, 384, pressed),
        1,
        "`Pool::heap` frees the bytes"
    );
    assert_eq!(
        freed(Pool::erased, 384, pressed),
        0,
        "a dropped `Pool::erased` erases a block whose end touches a released span"
    );
}

/// Gives the count of freed allocations that hold `PLAIN`, after `case` runs on the
/// pool that `pool` gives for `budget` and drops it.
fn freed(pool: fn(Config) -> Pool, budget: u64, case: impl FnOnce(Pool)) -> u64 {
    let pool = pool(Config::new(budget).expect("the budget fits"));
    let ((), found) = ALLOCATOR.freed_holding(&PLAIN, || case(pool));
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

/// Puts `PLAIN` in a block, purges its class, and carves the class again with a block
/// that ends before `PLAIN`.
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

/// Carves class 2, then two blocks of class 1, whose second ends at the start of the
/// released span of class 2 and holds `PLAIN` at its end.
fn pressed(pool: Pool) {
    drop(pool.alloc(192).expect("the budget has room"));
    let mut first = pool.alloc(128).expect("the budget has room");
    first.fill(0x11);
    let mut second = pool.alloc(128).expect("the budget has room");
    second.fill(0x11);
    second[128 - PLAIN.len()..].copy_from_slice(&PLAIN);
    drop(pool);
    drop(first);
    drop(second);
}
