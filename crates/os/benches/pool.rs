//! A `block` pool over OS memory under pressure: the system calls of each alloc and
//! the first write of each page it gives back.

use block::{Config, Pool};
use divan::Bencher;
use os::memory::Memory;

const SMALL: usize = 8000;
const LARGE: usize = 16_384;

fn main() {
    divan::main();
}

fn create_pool(budget: usize) -> Pool {
    let budget = u64::try_from(budget).expect("a usize fits in a u64");
    let config = Config::new(budget).expect("the budget fits");
    let memory = Memory::new(config.reservation()).expect("the system has the range");
    Pool::new(config, memory)
}

/// Blocks of two sizes in turn, with a budget for one: each alloc purges the range of
/// the other size, commits its own again, and writes it.
#[divan::bench]
fn alloc_under_pressure(bencher: Bencher<'_, '_>) {
    let pool = create_pool(block::footprint(LARGE));
    let mut small = false;
    bencher.bench_local(|| {
        small = !small;
        let len = if small { SMALL } else { LARGE };
        pool.alloc(len).expect("the budget has room").fill(1);
    });
}

/// Blocks of two sizes in turn, with a budget for two large blocks while one of them
/// stays in use: the small alloc discards the payload pages of the free large block,
/// and the large alloc purges the small size and takes that block back. Each alloc
/// writes its block.
#[divan::bench]
fn alloc_cold(bencher: Bencher<'_, '_>) {
    let pool = create_pool(2 * block::footprint(LARGE));
    let held = pool.alloc(LARGE).expect("the budget has room");
    drop(pool.alloc(LARGE).expect("the budget has room"));
    let mut small = false;
    bencher.bench_local(|| {
        small = !small;
        let len = if small { SMALL } else { LARGE };
        pool.alloc(len)
            .expect("the free block gives its pages")
            .fill(1);
    });
    drop(held);
}
