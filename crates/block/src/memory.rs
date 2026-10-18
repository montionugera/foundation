//! The address space a pool cuts its blocks from.

use std::alloc::{Layout, alloc, dealloc};
use std::cell::{Cell, RefCell};
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
    /// multiples of it from [`base`](Self::base). The pool credits its budget by the
    /// whole pages in a range it discards. A memory whose discard gives back pages
    /// gives back at least those pages. [`Heap`] and [`Erased`] give back none.
    fn page(&self) -> usize;
}

/// The system has no memory to commit a range now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

/// Memory from the heap. A commit writes zero over each page of its range that no
/// commit got before, so a pool on it touches only the pages that it commits. It
/// never gives pages back, and acts as if its pages were 4096 bytes. It serves tests
/// and simulation, and makes no OS call.
pub struct Heap {
    base: NonNull<u8>,
    /// Whole pages, so a commit writes zero over whole pages.
    layout: Layout,
    len: usize,
    /// One bit for each page, set when a commit got the page.
    got: Box<[Cell<u64>]>,
}

impl Heap {
    /// Allocates `len` bytes aligned to [`ALIGN`], and writes zero over the first
    /// page.
    ///
    /// # Panics
    ///
    /// If `len` is 0 or too large to allocate.
    #[must_use]
    pub fn new(len: usize) -> Self {
        assert!(len > 0, "heap memory must be more than 0 bytes");
        let too_large =
            || -> ! { panic!("heap memory of {len} bytes is too large to allocate") };
        let size = len.checked_next_multiple_of(PAGE);
        let Some(Ok(layout)) = size.map(|size| Layout::from_size_align(size, ALIGN))
        else {
            too_large()
        };
        // SAFETY: the layout has a size above 0.
        let Some(base) = NonNull::new(unsafe { alloc(layout) }) else {
            too_large()
        };
        let got = iter::repeat_with(Cell::default)
            .take(len.div_ceil(PAGE).div_ceil(64))
            .collect();
        let heap = Self {
            base,
            layout,
            len,
            got,
        };
        heap.commit(0, 1).expect("heap memory commits");
        heap
    }
}

// SAFETY: `Heap` owns its allocation, and the allocator lets any thread free it.
unsafe impl Send for Heap {}

// SAFETY: `new` commits the first page, and `commit` writes zero over each page of
// its range that `got` does not hold, so each byte of a committed range is
// initialized. The allocation stays at one address until the drop, and `purge` and
// `discard` change nothing.
unsafe impl Memory for Heap {
    fn base(&self) -> NonNull<u8> {
        self.base
    }

    fn len(&self) -> usize {
        self.len
    }

    fn commit(&self, offset: usize, len: usize) -> Result<(), Refused> {
        let end = offset.checked_add(len).filter(|&end| end <= self.len());
        let end = end.expect("a commit lies in the memory");
        let pages = offset / PAGE..end.div_ceil(PAGE);
        // A word at a time, so a commit of pages that it got costs a load per 64.
        for index in pages.start / 64..pages.end.div_ceil(64) {
            let below = |page: usize| match page - index * 64 {
                64 => u64::MAX,
                bit => (1 << bit) - 1,
            };
            let range = below(pages.end.min(index * 64 + 64))
                & !below(pages.start.max(index * 64));
            let word = &self.got[index];
            let mut fresh = range & !word.get();
            word.set(word.get() | range);
            while fresh != 0 {
                let start = (index * 64 + fresh.trailing_zeros() as usize) * PAGE;
                fresh &= fresh - 1;
                // SAFETY: the page starts under `len`.
                let page = unsafe { self.base.add(start) };
                // SAFETY: the allocation holds whole pages.
                unsafe { page.write_bytes(0, PAGE) };
            }
        }
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
        // SAFETY: `new` got `base` from the allocator with this layout.
        unsafe { dealloc(self.base.as_ptr(), self.layout) };
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
    /// Allocates `len` bytes aligned to [`ALIGN`], as [`Heap::new`] does.
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

// SAFETY: each method is the one of `heap`, and `commit` also records its range.
unsafe impl Memory for Erased {
    fn base(&self) -> NonNull<u8> {
        self.heap.base()
    }

    fn len(&self) -> usize {
        self.heap.len()
    }

    fn commit(&self, offset: usize, len: usize) -> Result<(), Refused> {
        self.heap.commit(offset, len)?;
        let end = offset + len;
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
            // SAFETY: a commit of `heap` initialized the bytes, and nothing else holds
            // them at the drop.
            let bytes =
                unsafe { slice::from_raw_parts_mut(start.as_ptr(), range.len()) };
            bytes.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Heap memory of `len` bytes with `0xaa` in each byte after the first page, as
    /// memory that the allocator reuses can hold.
    fn dirty(len: usize) -> Heap {
        let heap = Heap::new(len);
        // SAFETY: the second page starts in the allocation.
        let rest = unsafe { heap.base().add(PAGE) };
        // SAFETY: the bytes lie in the allocation, and no commit holds them yet.
        unsafe { rest.write_bytes(0xaa, len - PAGE) };
        heap
    }

    /// The bytes of `heap` in `range`.
    fn bytes(heap: &Heap, range: Range<usize>) -> Vec<u8> {
        // SAFETY: the range lies in the allocation.
        let start = unsafe { heap.base().add(range.start) };
        // SAFETY: `new`, `dirty`, or a commit initialized each byte.
        unsafe { slice::from_raw_parts(start.as_ptr(), range.len()) }.to_vec()
    }

    #[test]
    fn a_heap_commit_writes_zero_over_each_page_that_no_commit_got() {
        let len = 70 * PAGE + 100;
        let heap = dirty(len);
        // SAFETY: the first byte lies in the first page, which `new` commits.
        unsafe { heap.base().write(7) };
        heap.commit(100, 50).expect("heap memory commits");
        assert_eq!(bytes(&heap, 0..1), [7], "the first page keeps its bytes");
        heap.commit(PAGE + 10, PAGE).expect("heap memory commits");
        assert_eq!(
            bytes(&heap, PAGE..3 * PAGE),
            [0; 2 * PAGE],
            "the pages it got"
        );
        assert_eq!(
            bytes(&heap, 3 * PAGE..4 * PAGE),
            [0xaa; PAGE],
            "a later page"
        );
        // SAFETY: the byte lies in the allocation.
        let byte = unsafe { heap.base().add(PAGE + 5) };
        // SAFETY: the byte lies in a page that a commit got.
        unsafe { byte.write(9) };
        heap.commit(PAGE, 1).expect("heap memory commits");
        assert_eq!(
            bytes(&heap, PAGE + 5..PAGE + 6),
            [9],
            "a page it got before"
        );
        heap.commit(65 * PAGE, 1).expect("heap memory commits");
        assert_eq!(
            bytes(&heap, 65 * PAGE..66 * PAGE),
            [0; PAGE],
            "a page of word 1"
        );
        // SAFETY: the byte lies in the allocation.
        let byte = unsafe { heap.base().add(65 * PAGE) };
        // SAFETY: the byte lies in a page that a commit got.
        unsafe { byte.write(9) };
        heap.commit(62 * PAGE + 1, 5 * PAGE - 2)
            .expect("heap memory commits");
        assert_eq!(
            bytes(&heap, 62 * PAGE..65 * PAGE),
            [0; 3 * PAGE],
            "the pages to the end of word 0, and the first of word 1"
        );
        heap.commit(65 * PAGE, 1).expect("heap memory commits");
        assert_eq!(bytes(&heap, 65 * PAGE..65 * PAGE + 1), [9], "a page it got");
        assert_eq!(
            bytes(&heap, 66 * PAGE..67 * PAGE),
            [0; PAGE],
            "the last page"
        );
        assert_eq!(
            [
                bytes(&heap, 62 * PAGE - 1..62 * PAGE),
                bytes(&heap, 67 * PAGE..67 * PAGE + 1)
            ],
            [[0xaa], [0xaa]],
            "the pages on each side"
        );
        heap.commit(len - 1, 1).expect("heap memory commits");
        assert_eq!(bytes(&heap, 70 * PAGE..len), [0; 100], "the last part page");
    }

    #[test]
    #[should_panic(expected = "a commit lies in the memory")]
    fn a_heap_panics_on_a_commit_past_its_end() {
        Heap::new(PAGE)
            .commit(PAGE - 1, 2)
            .expect("the commit panics first");
    }

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
