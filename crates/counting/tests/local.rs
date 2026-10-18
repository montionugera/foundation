//! As the global allocator, `count` counts only the allocations of its thread, also
//! while another thread allocates. This binary has no test harness, so that the
//! only other thread is the one that it starts.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[cfg(unix)]
#[global_allocator]
static ALLOCATOR: counting::Local = counting::Local::new();

#[cfg(unix)]
#[expect(
    clippy::disallowed_methods,
    reason = "a thread test; `counting` has no `env`"
)]
fn main() {
    use std::hint::black_box;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::thread;

    let (stopped, allocations) = (AtomicBool::new(false), AtomicU64::new(0));
    thread::scope(|scope| {
        scope.spawn(|| {
            while !stopped.load(Relaxed) {
                drop(black_box(Box::new(1)));
                allocations.fetch_add(1, Relaxed);
            }
        });
        let (block, counted) = ALLOCATOR.count(|| {
            let start = allocations.load(Relaxed);
            while allocations.load(Relaxed) < start + 1_000 {
                std::hint::spin_loop();
            }
            black_box(Box::new(4))
        });
        stopped.store(true, Relaxed);
        assert_eq!(
            (*block, counted),
            (4, 1),
            "only the box of this thread counts"
        );
    });
}

#[cfg(not(unix))]
fn main() {}
