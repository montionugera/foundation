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

/// A reserved range of address space that one [`Pool`](crate::Pool) cuts blocks from.
///
/// The first [`ALIGN`] bytes are usable from the start. The pool calls
/// [`commit`](Self::commit) before it uses any other range, first or after a purge,
/// and [`purge`](Self::purge) when a range is idle. Offsets are from
/// [`base`](Self::base). A range need not be page-aligned: `commit` rounds it out, and
/// `purge` rounds it in.
///
/// # Safety
///
/// - `base` and `len` give the same values on each call. The `len` bytes at `base`
///   belong to this value alone, at one address, until it drops.
/// - The first [`ALIGN`] bytes are as if a `commit` of them returned `Ok` before the
///   first call.
/// - When `commit` returns `Ok`, each byte in its range is readable, writable, and
///   initialized. It stays so until a `purge` of that byte or the drop.
/// - A `purge` may change the bytes in its range. It changes no other byte.
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
}

/// The system has no memory to commit a range now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// Memory from the heap, usable in full from the start. It never gives pages back.
/// It serves tests and simulation, where no OS mapping exists.
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
// stays at one address until the drop, and `purge` changes nothing.
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
pub struct Erased {
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
    pub fn new(len: usize) -> Self {
        Self {
            heap: Heap::new(len),
            committed: RefCell::new(iter::once(0..ALIGN.min(len)).collect()),
        }
    }
}

// SAFETY: `base` and `len` are those of `heap`, whose bytes are usable from the start,
// and `commit` and `purge` change no byte.
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
        // touches the one before it, and the record stays at one range per class.
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
}

impl fmt::Debug for Erased {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Erased").field("len", &self.len()).finish()
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
    use crate::{Config, Pool};

    fn committed(memory: &Erased) -> Vec<Range<usize>> {
        memory.committed.borrow().clone()
    }

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
    fn erased_prints_its_length() {
        assert_eq!(format!("{:?}", Erased::new(64)), "Erased { len: 64 }");
    }

    #[test]
    #[should_panic(expected = "a commit lies in the memory")]
    fn erased_panics_on_a_commit_past_its_end() {
        let memory = Erased::new(4096);
        memory.commit(4000, 97).expect("the commit panics first");
    }

    #[test]
    fn erased_erases_a_pool_that_drops_before_its_block() {
        let config = Config::new(1 << 14).expect("the budget fits");
        let memory = Erased::new(config.reservation());
        let pool = Pool::new(config, memory);
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
