//! A pool on `Heap::erased` leaves the bytes of its blocks in no freed allocation. The
//! scan covers every thread, so this binary has no test harness.

#![expect(unsafe_code, reason = "a scanning allocator implements `GlobalAlloc`")]
#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::alloc::{GlobalAlloc, Layout, System};
use std::slice;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;

use block::{ALIGN, Config, Heap, Memory, Pool};

const PLAIN: [u8; 32] = [0x5a; 32];

/// Counts the frees of allocations of `size` bytes that hold `PLAIN`.
struct Scanning {
    size: AtomicUsize,
    held: AtomicU64,
}

// SAFETY: every call goes to `System` unchanged.
unsafe impl GlobalAlloc for Scanning {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if layout.size() == self.size.load(Relaxed) {
            // SAFETY: only a `Heap` allocates this size here, through
            // `alloc_zeroed`, so each byte is initialized.
            let bytes = unsafe { slice::from_raw_parts(ptr, layout.size()) };
            if bytes.windows(PLAIN.len()).any(|window| window == PLAIN) {
                self.held.fetch_add(1, Relaxed);
            }
        }
        // SAFETY: the caller keeps the contract of `GlobalAlloc::dealloc`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Scanning = Scanning {
    size: AtomicUsize::new(0),
    held: AtomicU64::new(0),
};

fn main() {
    for (len, at) in [(32, 0), (3000, 1), (3000, 0)] {
        let held = freed(Heap::new, len, at);
        assert_eq!(
            held, 1,
            "a dropped `Heap::new` frees the bytes ({len}, {at})"
        );
        let held = freed(Heap::erased, len, at);
        assert_eq!(
            held, 0,
            "a dropped `Heap::erased` erases them ({len}, {at})"
        );
    }
}

/// Drops a pool on the memory of `heap` while it lends two blocks of different size
/// classes, one of 32 bytes and one of `len` bytes, then the blocks. Block `at` holds
/// `PLAIN` at its end. Gives the count of freed allocations that hold `PLAIN`.
fn freed(heap: fn(usize) -> Heap, len: usize, at: usize) -> u64 {
    let config = Config::new(1 << 14).expect("the budget fits");
    let memory = heap(config.reservation());
    ALLOCATOR.size.store(memory.len() + ALIGN, Relaxed);
    let before = ALLOCATOR.held.load(Relaxed);
    let pool = Pool::new(config, memory);
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
    drop(blocks);
    ALLOCATOR.held.load(Relaxed) - before
}
