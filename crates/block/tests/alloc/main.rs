//! The hot path makes no heap allocation. This binary has no test harness: the count
//! covers each thread, and a harness allocates on its own thread at any time.

#![expect(unsafe_code, reason = "a counting allocator implements `GlobalAlloc`")]
#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering::Relaxed;

use block::{Config, Heap, Pool};

/// Counts the allocations, the bytes of zeroed allocations, and the frees of the
/// process.
struct Counting {
    count: AtomicU64,
    zeroed: AtomicU64,
    freed: AtomicU64,
}

// SAFETY: every call goes to `System` unchanged.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.count.fetch_add(1, Relaxed);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        self.count.fetch_add(1, Relaxed);
        self.zeroed.fetch_add(layout.size() as u64, Relaxed);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc_zeroed`.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        self.freed.fetch_add(1, Relaxed);
        // SAFETY: the caller keeps the contract of `GlobalAlloc::dealloc`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting {
    count: AtomicU64::new(0),
    zeroed: AtomicU64::new(0),
    freed: AtomicU64::new(0),
};

/// Allocations the process makes while `f` runs.
fn count(f: impl FnOnce()) -> u64 {
    let before = ALLOCATOR.count.load(Relaxed);
    f();
    ALLOCATOR.count.load(Relaxed) - before
}

fn main() {
    assert_eq!(count(|| drop(Box::new(1_u8))), 1, "the allocator counts");
    heap();

    let config = Config::new(1 << 16).expect("the budget fits");
    let heap = Heap::new(config.reservation());
    let pool = Pool::new(config, heap);
    let allocations = count(|| {
        for len in [0, 1, 64, 65, 1000, 4096].into_iter().cycle().take(600) {
            let mut unique = pool.alloc(len).expect("the budget has room");
            unique.fill(1);
            let block = unique.freeze();
            let clone = block.clone();
            assert_eq!(clone.len(), len, "a clone has the same bytes");
            drop(block);
            drop(clone);
            drop(pool.alloc(len).expect("the budget has room"));
        }
    });
    assert_eq!(allocations, 0, "the hot path allocated");
    let bytes = [7_u8; 4096];
    let allocations = count(|| {
        for len in [0, 1, 64, 65, 1000, 4096].into_iter().cycle().take(600) {
            let block = pool.copy(&bytes[..len]).expect("the budget has room");
            assert_eq!(&*block, &bytes[..len], "a copy holds the bytes");
        }
    });
    assert_eq!(allocations, 0, "a copy allocated");
    // One block of each of the four classes: a dropped block serves the next alloc.
    let committed = 128 + 192 + 1088 + 4160;
    assert_eq!(pool.committed(), committed, "blocks are used again");

    let config = Config::new(2 * 4160).expect("the budget fits");
    let heap = Heap::new(config.reservation());
    let pool = Pool::new(config, heap);
    let large = [pool.alloc(4096), pool.alloc(4096)].map(|block| block.expect("room"));
    drop(large);
    let allocations = count(|| {
        for _ in 0..64 {
            drop(pool.alloc(1).expect("a freed large block gives its budget"));
        }
    });
    assert_eq!(allocations, 0, "the pressure path allocated");
    assert_eq!(pool.committed(), 128, "the idle large size gave its budget");

    let allocations = count(|| {
        assert_eq!(
            pool.purge(),
            0,
            "the small size returned a block this interval"
        );
        assert_eq!(pool.purge(), 128, "the small size stayed idle");
    });
    assert_eq!(allocations, 0, "the purge path allocated");
    assert_eq!(pool.committed(), 0, "each idle size gave its pages back");
    cold_block();
}

/// A heap allocates once for its bytes and once for its page bits, and touches only
/// the pages that it commits.
fn heap() {
    let (freed, zeroed) = (
        ALLOCATOR.freed.load(Relaxed),
        ALLOCATOR.zeroed.load(Relaxed),
    );
    let allocations = count(|| drop(Heap::new(1 << 20)));
    assert_eq!(
        allocations, 2,
        "a heap allocates its bytes and its page bits"
    );
    assert_eq!(
        ALLOCATOR.freed.load(Relaxed) - freed,
        2,
        "a heap frees both"
    );
    // A release build may get the page bits zeroed from the allocator.
    let bits = (1 << 20) / 4096 / 8;
    assert!(
        ALLOCATOR.zeroed.load(Relaxed) - zeroed <= bits,
        "a heap writes zero only over the pages that it commits"
    );
    #[cfg(all(target_os = "linux", not(miri)))]
    untouched();
}

/// A new heap of 256 MiB adds fewer than 1024 pages to the process in memory, as
/// `/proc/self/statm` counts them.
#[cfg(all(target_os = "linux", not(miri)))]
fn untouched() {
    let before = resident();
    let heap = Heap::new(1 << 28);
    assert!(
        resident().saturating_sub(before) < 1024,
        "a heap touches only the pages that it commits"
    );
    drop(heap);
}

/// The pages of the process in memory, from `/proc/self/statm`.
#[cfg(all(target_os = "linux", not(miri)))]
fn resident() -> u64 {
    let statm = std::fs::read_to_string("/proc/self/statm").expect("Linux gives statm");
    let pages = statm
        .split(' ')
        .nth(1)
        .expect("statm gives the resident pages");
    pages.parse().expect("the resident pages are a number")
}

/// A free large block gives its pages to a small alloc, and the large alloc takes it
/// back.
fn cold_block() {
    let config = Config::new(2 * 16_448).expect("the budget fits");
    let heap = Heap::new(config.reservation());
    let pool = Pool::new(config, heap);
    let held = pool.alloc(16_384).expect("the budget has room");
    drop(pool.alloc(16_384).expect("the budget has room"));
    let allocations = count(|| {
        for len in [8000, 16_384].into_iter().cycle().take(64) {
            drop(pool.alloc(len).expect("the free block gives its pages"));
        }
    });
    assert_eq!(allocations, 0, "the path of a cold block allocated");
    drop(held);
}
