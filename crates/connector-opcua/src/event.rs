//! The event loop that open62541 runs on. It reads an injected clock and wall time,
//! runs the timers and the delayed callbacks of the copy, and does no I/O.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::ffi::c_void;
use std::ptr::NonNull;

use env::clock::Clock;
use env::rng::Rng;
use types::time::{Monotonic, Stamp};

use crate::alloc;
use crate::ffi;

/// The wall time of a loop, which open62541 puts in each request header and compares
/// with the time of each channel token. The loop calls it inside each call of its
/// client or server, so it must not borrow what such a call borrows. A panic in it
/// aborts the process.
pub(crate) type Wall = Box<dyn Fn() -> Stamp>;

/// Gives the monotonic time of `clock` as the wall time, from the Unix epoch, for a
/// loop of a test or a benchmark, which has no wall time of a node.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn monotonic(clock: Clock) -> Wall {
    Box::new(move || {
        let nanos = i64::try_from(clock.now().0).expect("a clock under 292 years");
        Stamp::from_nanos(nanos)
    })
}

/// What the C loop reads.
struct Time {
    clock: Clock,
    wall: Wall,
}

/// An open62541 event loop, on the thread that made it.
pub(crate) struct Loop {
    raw: NonNull<ffi::EventLoop>,
    /// The C loop reads it until `drop` frees the loop.
    time: NonNull<Time>,
}

impl Loop {
    /// Makes a loop whose monotonic time is `clock` and whose wall time is `wall`. It
    /// sets the random values of open62541 on this thread from `rng`, so a run on a
    /// simulated clock replays. Each loop on this thread then uses them, so give each
    /// loop on a thread an `rng` whose next value differs.
    ///
    /// # Aborts
    ///
    /// When the C allocation of the loop fails, as [`alloc::exhausted`] says.
    pub(crate) fn new(clock: Clock, wall: Wall, rng: &mut Rng) -> Self {
        // SAFETY: it writes only the generator of this thread.
        unsafe { ffi::UA_random_seed_deterministic(rng.next_u64()) };
        let time = NonNull::from(Box::leak(Box::new(Time { clock, wall })));
        // SAFETY: `now` and `wall_now` read the `Time` at `time`, which lives until
        // `drop`.
        let raw = unsafe { ffi::shim_loop_new(now, wall_now, time.as_ptr().cast()) };
        let Some(raw) = NonNull::new(raw) else {
            alloc::exhausted()
        };
        Self { raw, time }
    }

    /// Gives the loop for a client or server config with `externalEventLoop`. Delete
    /// that client or server before the loop drops.
    pub(crate) fn raw(&self) -> *mut ffi::EventLoop {
        self.raw.as_ptr()
    }

    /// Gives the members of the loop, each of which takes `raw`.
    pub(crate) fn members(&self) -> &ffi::EventLoop {
        // SAFETY: the loop lives as long as `self`, and C writes it only in a call
        // that takes `raw`, which no shared borrow spans.
        unsafe { self.raw.as_ref() }
    }

    /// Gives the due time of the next timer, now rounded down to 100 ns when a delayed
    /// callback waits, or `None` when nothing waits or the next timer is due after the
    /// clock ends. A due time before the clock's epoch comes as the epoch: a once timer
    /// may have a date that has passed.
    pub(crate) fn next(&self) -> Option<Monotonic> {
        // SAFETY: the member takes its own loop.
        let ticks = unsafe { (self.members().next_timer)(self.raw()) };
        // Also `None` for the `i64::MAX` of an empty loop.
        ticks.max(0).unsigned_abs().checked_mul(100).map(Monotonic)
    }

    /// Runs the timers and the delayed callbacks that are due, with no wait.
    ///
    /// # Panics
    ///
    /// If open62541 fails the run.
    pub(crate) fn run(&self) {
        // SAFETY: the member takes its own loop.
        let status = ffi::Status(unsafe { (self.members().run)(self.raw(), 0) });
        assert_eq!(status, ffi::Status::GOOD, "open62541 failed a run");
    }

    /// Gives whether a delayed callback waits for the next run.
    #[cfg_attr(
        not(any(test, feature = "sim")),
        expect(dead_code, reason = "the stop of a server of #435 calls it")
    )]
    pub(crate) fn delayed(&self) -> bool {
        // SAFETY: the loop is one of `shim_loop_new`.
        unsafe { ffi::shim_loop_delayed(self.raw()) }
    }
}

impl Drop for Loop {
    /// Runs the queued delayed callbacks, which free what they hold, and those that
    /// they queue, and frees the loop. Aborts the process when callbacks still wait
    /// after 64 passes.
    fn drop(&mut self) {
        // SAFETY: the loop lives, and `raw` tells the caller to delete each client
        // and server on it first.
        unsafe { ffi::shim_loop_free(self.raw.as_ptr()) };
        // SAFETY: `new` leaked the box, and the loop that read it is gone.
        drop(unsafe { Box::from_raw(self.time.as_ptr()) });
    }
}

/// Gives the monotonic time of the `Time` at `time` in ticks of 100 ns.
///
/// # Safety
///
/// `time` points at a live `Time`.
unsafe extern "C" fn now(time: *mut c_void) -> i64 {
    // SAFETY: the caller keeps the time live.
    let time = unsafe { &*time.cast::<Time>() };
    i64::try_from(time.clock.now().0 / 100)
        .expect("invariant: u64::MAX / 100 fits in i64")
}

/// Gives the wall time of the `Time` at `time` in ticks of 100 ns from the Unix
/// epoch, rounded down.
///
/// # Safety
///
/// `time` points at a live `Time`.
unsafe extern "C" fn wall_now(time: *mut c_void) -> i64 {
    // SAFETY: the caller keeps the time live.
    let time = unsafe { &*time.cast::<Time>() };
    (time.wall)().nanos().div_euclid(100)
}

#[cfg(test)]
mod tests;
