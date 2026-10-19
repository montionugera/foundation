//! The [`Local`] allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::atomic::{AtomicU64, AtomicUsize};

/// An allocator that gets its memory from [`System`] and counts the allocations of
/// the thread that runs [`Local::count`], as [`Allocator`](crate::Allocator) counts
/// them. The allocations of other threads do not count, so a runtime or a harness on
/// another thread can run while it counts.
///
/// ```
/// #[global_allocator]
/// static ALLOCATOR: counting::Local = counting::Local::new();
///
/// fn main() {
///     let (block, allocations) = ALLOCATOR.count(|| Box::new(4));
///     assert_eq!((*block, allocations), (4, 1));
/// }
/// ```
pub struct Local {
    /// The thread of the call of [`Local::count`] that runs, or [`NONE`].
    thread: AtomicUsize,
    allocations: AtomicU64,
}

/// No call runs. No thread has it as its `pthread_t`: Linux and macOS give a pointer,
/// and Miri gives a small count from 0.
const NONE: usize = usize::MAX;

impl Local {
    /// Returns an allocator that counts nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            thread: AtomicUsize::new(NONE),
            allocations: AtomicU64::new(0),
        }
    }

    /// Runs `f` and returns its result and the allocations made through this allocator
    /// on the calling thread while it ran.
    ///
    /// # Panics
    ///
    /// If another call runs.
    pub fn count<T>(&self, f: impl FnOnce() -> T) -> (T, u64) {
        let idle = self
            .thread
            .compare_exchange(NONE, current(), Acquire, Relaxed);
        assert!(idle.is_ok(), "calls to `Local::count` do not overlap");
        let running = Running(self);
        let value = f();
        let allocations = self.allocations.load(Relaxed);
        drop(running);
        (value, allocations)
    }

    /// Counts `ptr` unless it is null or another thread allocated it, and returns it.
    fn counted(&self, ptr: *mut u8) -> *mut u8 {
        let thread = self.thread.load(Relaxed);
        if !ptr.is_null() && thread != NONE && thread == current() {
            self.allocations.fetch_add(1, Relaxed);
        }
        ptr
    }
}

/// Ends a call of [`Local::count`], also when its closure panics: it zeroes the count,
/// then frees the allocator for the next call.
struct Running<'a>(&'a Local);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.allocations.store(0, Relaxed);
        self.0.thread.store(NONE, Release);
    }
}

/// The `pthread_t` of the calling thread, which makes no allocation and no syscall.
#[cfg_attr(
    all(target_os = "linux", target_pointer_width = "64"),
    expect(
        clippy::cast_possible_truncation,
        reason = "a `pthread_t` on Linux is a `c_ulong`, of the width of a pointer"
    )
)]
#[cfg_attr(
    target_vendor = "apple",
    expect(
        trivial_numeric_casts,
        reason = "a `pthread_t` on macOS is a `uintptr_t`"
    )
)]
fn current() -> usize {
    // SAFETY: `pthread_self` has no precondition.
    let thread = unsafe { libc::pthread_self() };
    thread as usize
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Local {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Local")
            .field("allocations", &self.allocations)
            .finish_non_exhaustive()
    }
}

// SAFETY: every block comes from `System`, and each method calls the `System` method
// of its name.
unsafe impl GlobalAlloc for Local {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc`.
        self.counted(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::alloc_zeroed`.
        self.counted(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::realloc`, and every
        // pointer this allocator returns comes from `System`.
        self.counted(unsafe { System.realloc(ptr, layout, new_size) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller keeps the contract of `GlobalAlloc::dealloc`, and every
        // pointer this allocator returns comes from `System`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[cfg(test)]
mod tests {
    use std::{panic, thread};

    use super::*;

    const LAYOUT: Layout = Layout::new::<[u64; 8]>();

    /// A block of `LAYOUT` from `local`, on the calling thread.
    fn allocated(local: &Local) -> *mut u8 {
        // SAFETY: the layout is not empty.
        let ptr = unsafe { local.alloc(LAYOUT) };
        assert!(!ptr.is_null(), "the system has no memory for 64 bytes");
        ptr
    }

    fn free(local: &Local, ptr: *mut u8) {
        // SAFETY: `local` returned `ptr` for `LAYOUT`, and nothing uses it after.
        unsafe { local.dealloc(ptr, LAYOUT) }
    }

    #[test]
    fn counts_each_allocation_of_the_calling_thread() {
        let local = Local::new();
        let (ptr, allocations) = local.count(|| {
            let ptr = allocated(&local);
            // SAFETY: the layout is not empty.
            let zeroed = unsafe { local.alloc_zeroed(LAYOUT) };
            assert!(!zeroed.is_null(), "the system has no memory for 64 bytes");
            free(&local, zeroed);
            // SAFETY: `local` returned `ptr` for `LAYOUT`, and 128 rounded up to the
            // alignment does not pass `isize::MAX`.
            unsafe { local.realloc(ptr, LAYOUT, 128) }
        });
        assert_eq!(allocations, 3);
        assert!(!ptr.is_null(), "the system has no memory for 128 bytes");
        let layout = Layout::from_size_align(128, LAYOUT.align())
            .expect("invariant: 128 bytes at the alignment of `u64` is a layout");
        // SAFETY: `local` returned `ptr` for `layout`, and nothing uses it after.
        unsafe { local.dealloc(ptr, layout) }
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "a thread test; `counting` has no `env`"
    )]
    fn does_not_count_an_allocation_of_another_thread() {
        let local = Local::new();
        let ((), allocations) = local.count(|| {
            thread::scope(|scope| {
                let other = scope.spawn(|| free(&local, allocated(&local)));
                other.join().expect("the allocation does not panic");
            });
        });
        assert_eq!(allocations, 0);
    }

    #[test]
    fn does_not_count_an_allocation_outside_the_call() {
        let local = Local::new();
        free(&local, allocated(&local));
        assert_eq!(local.count(|| ()), ((), 0));
    }

    #[test]
    fn ends_the_call_when_the_closure_panics() {
        let local = Local::new();
        panic::catch_unwind(|| local.count(|| panic!("the closure")))
            .expect_err("the closure panics");
        let ((), allocations) = local.count(|| free(&local, allocated(&local)));
        assert_eq!(allocations, 1);
    }

    #[test]
    fn a_call_after_a_panic_counts_only_its_own_allocations() {
        let local = Local::new();
        panic::catch_unwind(|| {
            local.count(|| {
                free(&local, allocated(&local));
                panic!("the closure")
            })
        })
        .expect_err("the closure panics");
        assert_eq!(local.count(|| ()), ((), 0));
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "a thread test; `counting` has no `env`"
    )]
    fn each_call_of_two_threads_counts_only_its_own_allocation_or_panics() {
        const CALLS: usize = if cfg!(miri) { 20 } else { 100_000 };
        let local = Local::new();
        let prior = panic::take_hook();
        panic::set_hook(Box::new(|_| {}));
        let wrong = thread::scope(|scope| {
            let caller = || {
                let call = || local.count(|| free(&local, allocated(&local)));
                let counts = (0..CALLS).map(|_| panic::catch_unwind(call));
                counts
                    .filter(|count| matches!(count, Ok(((), n)) if *n != 1))
                    .count()
            };
            let callers = [scope.spawn(caller), scope.spawn(caller)];
            callers.map(|caller| caller.join().expect("each panic is caught"))
        });
        panic::set_hook(prior);
        assert_eq!(wrong, [0, 0]);
    }

    #[test]
    #[should_panic(expected = "calls to `Local::count` do not overlap")]
    fn panics_on_a_call_inside_a_call() {
        let local = Local::new();
        local.count(|| local.count(|| ()));
    }

    #[test]
    fn returns_the_result_of_the_closure() {
        assert_eq!(Local::new().count(|| "result"), ("result", 0));
    }

    #[test]
    fn shows_the_count_of_a_call() {
        let local = Local::new();
        local.allocations.store(2, Relaxed);
        assert_eq!(format!("{local:?}"), "Local { allocations: 2, .. }");
    }
}
