//! The address space a pool cuts its blocks from.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::cell::RefCell;
use std::fmt;
use std::iter;
use std::ops::Range;
use std::ptr::NonNull;
use std::slice;

use zeroize::Zeroize;

use crate::ALIGN;

/// The page of [`Heap`]: the smallest page of the hosts that Foundation runs on.
const PAGE: usize = 4096;

/// A reserved range of address space that one [`Pool`](crate::Pool) cuts blocks from.
///
/// The first [`ALIGN`] bytes are usable from the start. The pool calls
/// [`commit`](Self::commit) before it uses any other range, first or after a purge,
/// [`purge`](Self::purge) when a range is idle, and [`discard`](Self::discard) for
/// the payload of a free block that it keeps. Offsets are from [`base`](Self::base). A
/// range need not be page-aligned: `commit` rounds it out, and `purge` and `discard`
/// round it in.
///
/// # Safety
///
/// - `base` and `len` give the same values on each call. The `len` bytes at `base`
///   belong to this value alone, at one address, until it drops.
/// - The first [`ALIGN`] bytes are as if a `commit` of them returned `Ok` before the
///   first call.
/// - When `commit` returns `Ok`, each byte in its range is readable, writable, and
///   initialized. It stays so until a `purge` of that byte or the drop.
/// - A `purge` or a `discard` may change the bytes in its range. It changes no other
///   byte.
#[expect(
    clippy::len_without_is_empty,
    reason = "a reserved range is never empty"
)]
pub unsafe trait Memory: Send {
    /// The first byte of the range.
    fn base(&self) -> NonNull<u8>;

    /// The length of the range in bytes.
    fn len(&self) -> usize;

    /// Makes `len` bytes at `offset` usable.
    ///
    /// # Errors
    ///
    /// [`Refused`] when the system has no memory for the range now. The range stays
    /// unusable, and a later call may succeed.
    fn commit(&self, offset: usize, len: usize) -> Result<(), Refused>;

    /// Lets the system take back the pages that lie fully in `len` bytes at `offset`.
    /// Those pages then stop counting against the memory the system can commit.
    fn purge(&self, offset: usize, len: usize);

    /// Gives back the memory of the pages that lie fully in `len` bytes at `offset`,
    /// and keeps them committed: they stay readable and writable, with contents that
    /// are not known. It changes no protection, so it splits no mapping. The system
    /// can keep their commit charge.
    fn discard(&self, offset: usize, len: usize);

    /// The bytes of one page: a power of two of at least [`ALIGN`]. Pages start at
    /// multiples of it from [`base`](Self::base). The pool counts the whole pages in
    /// a range it discards as given back, and credits its budget by them.
    fn page(&self) -> usize;
}

/// The system has no memory to commit a range now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// Memory from the heap, usable in full from the start. It never gives pages back,
/// and acts as if its pages were 4096 bytes. It serves tests and simulation, where no
/// OS mapping exists.
pub struct Heap {
    /// What the allocator gave. `base` rounds it up to [`ALIGN`].
    allocation: NonNull<u8>,
    layout: Layout,
    base: NonNull<u8>,
}

impl Heap {
    /// Allocates `len` zeroed bytes aligned to [`ALIGN`].
    ///
    /// # Panics
    ///
    /// If `len` is 0 or too large to allocate.
    #[must_use]
    pub fn new(len: usize) -> Self {
        assert!(len > 0, "heap memory must be more than 0 bytes");
        let too_large =
            || -> ! { panic!("heap memory of {len} bytes is too large to allocate") };
        // Std reaches `calloc`, which leaves the pages untouched, only at the
        // allocator's own alignment. At `ALIGN` it writes zero over every page.
        let Some(layout) = len
            .checked_add(ALIGN)
            .and_then(|size| Layout::from_size_align(size, 1).ok())
        else {
            too_large()
        };
        // SAFETY: the layout has a size above 0.
        let allocation = unsafe { alloc_zeroed(layout) };
        let Some(allocation) = NonNull::new(allocation) else {
            too_large()
        };
        let addr = allocation.addr().get();
        let skew = addr.next_multiple_of(ALIGN) - addr;
        // SAFETY: `skew` is under `ALIGN`, so `base` and the `len` bytes after it
        // lie in the allocation.
        let base = unsafe { allocation.add(skew) };
        Self {
            allocation,
            layout,
            base,
        }
    }
}

// SAFETY: `Heap` owns its allocation, and the allocator lets any thread free it.
unsafe impl Send for Heap {}

// SAFETY: the allocation is zeroed, so each byte is initialized from the start. It
// stays at one address until the drop, and `purge` and `discard` change nothing.
unsafe impl Memory for Heap {
    fn base(&self) -> NonNull<u8> {
        self.base
    }

    fn len(&self) -> usize {
        self.layout.size() - ALIGN
    }

    fn commit(&self, _offset: usize, _len: usize) -> Result<(), Refused> {
        Ok(())
    }

    fn purge(&self, _offset: usize, _len: usize) {}

    fn discard(&self, _offset: usize, _len: usize) {}

    fn page(&self) -> usize {
        PAGE
    }
}

impl fmt::Debug for Heap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Heap").field("len", &self.len()).finish()
    }
}

impl Drop for Heap {
    fn drop(&mut self) {
        // SAFETY: `new` got `allocation` from the allocator with this layout.
        unsafe { dealloc(self.allocation.as_ptr(), self.layout) };
    }
}

/// Heap memory, as [`Heap`] gives it, for blocks that hold a secret. At the drop, it
/// writes zero over the first [`ALIGN`] bytes and over each range that
/// [`Memory::commit`] got, with writes that the compiler keeps, before it frees them.
pub(crate) struct Erased {
    heap: Heap,
    /// The first [`ALIGN`] bytes and the ranges that `commit` got.
    committed: RefCell<Vec<Range<usize>>>,
}

impl Erased {
    /// Allocates `len` zeroed bytes aligned to [`ALIGN`], as [`Heap::new`] does.
    ///
    /// # Panics
    ///
    /// If `len` is 0 or too large to allocate.
    #[must_use]
    pub(crate) fn new(len: usize) -> Self {
        Self {
            heap: Heap::new(len),
            committed: RefCell::new(iter::once(0..ALIGN.min(len)).collect()),
        }
    }
}

// SAFETY: `base` and `len` are those of `heap`, whose bytes are usable from the start,
// and `commit`, `purge`, and `discard` change no byte.
unsafe impl Memory for Erased {
    fn base(&self) -> NonNull<u8> {
        self.heap.base()
    }

    fn len(&self) -> usize {
        self.heap.len()
    }

    fn commit(&self, offset: usize, len: usize) -> Result<(), Refused> {
        let end = offset.checked_add(len).filter(|&end| end <= self.len());
        let end = end.expect("a commit lies in the memory");
        let mut committed = self.committed.borrow_mut();
        // A pool commits each class from the start of its span up, so a new range
        // mostly touches one in the record. A join keeps the union, so no committed
        // byte leaves the record.
        let touched = committed
            .iter_mut()
            .find(|range| range.start <= end && offset <= range.end);
        match touched {
            Some(range) => *range = range.start.min(offset)..range.end.max(end),
            None => committed.push(offset..end),
        }
        Ok(())
    }

    fn purge(&self, _offset: usize, _len: usize) {}

    fn discard(&self, _offset: usize, _len: usize) {}

    fn page(&self) -> usize {
        self.heap.page()
    }
}

impl Drop for Erased {
    fn drop(&mut self) {
        for range in self.committed.get_mut().drain(..) {
            // SAFETY: each range lies in the `len` bytes at `base`.
            let start = unsafe { self.heap.base().add(range.start) };
            // SAFETY: the bytes are initialized, since `Heap::new` zeroed them, and
            // nothing else holds them at the drop.
            let bytes =
                unsafe { slice::from_raw_parts_mut(start.as_ptr(), range.len()) };
            bytes.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed(memory: &Erased) -> Vec<Range<usize>> {
        memory.committed.borrow().clone()
    }

    /// Reads `committed`, because the first `ALIGN` bytes hold only the header of the
    /// region, which holds no byte of a block, so no caller can see their erase. A
    /// commit next to a range that adds a range in place of a join erases the same
    /// bytes, so a caller sees only a longer record.
    #[test]
    fn erased_records_the_first_bytes_and_merges_a_commit_that_touches_a_range() {
        let memory = Erased::new(4096);
        let first = slice::from_ref(&(0..ALIGN));
        assert_eq!(committed(&memory), first, "the first bytes");
        let commits = [
            ((1000, 100), [0..ALIGN, 1000..1100]),
            ((ALIGN, 100), [0..164, 1000..1100]),
            ((1100, 50), [0..164, 1000..1150]),
            ((950, 50), [0..164, 950..1150]),
            ((1000, 10), [0..164, 950..1150]),
        ];
        for ((offset, len), ranges) in commits {
            memory.commit(offset, len).expect("heap memory commits");
            assert_eq!(committed(&memory), ranges, "after a commit at {offset}");
        }
    }

    #[test]
    #[should_panic(expected = "a commit lies in the memory")]
    fn erased_panics_on_a_commit_past_its_end() {
        let memory = Erased::new(4096);
        memory.commit(4000, 97).expect("the commit panics first");
    }

    #[test]
    #[cfg(not(loom))]
    fn erased_erases_a_pool_that_drops_before_its_block() {
        use crate::{Config, Pool};

        let config = Config::new(1 << 14).expect("the budget fits");
        let pool = Pool::erased(config);
        let mut block = pool.alloc(3000).expect("the budget has room");
        block.fill(0x5a);
        let block = block.freeze();
        drop(pool);
        assert!(
            block.iter().all(|&byte| byte == 0x5a),
            "the block outlives its pool"
        );
    }
}
