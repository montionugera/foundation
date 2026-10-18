//! The lines of the logger of each loop and the aborts of `shim.c`, which C gives to
//! Rust: each line becomes an event of `tracing`, and each abort a panic.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::borrow::Cow;
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_void};

use env::clock::Clock;
use types::time::{Monotonic, Span};

/// The lines of each level that a loop writes in a window.
const LINES: u32 = 100;
/// The length of a window.
const WINDOW: Span = Span::SECOND;

/// The logger of a loop: it writes each line with the span of the loop as the parent,
/// and at most 100 lines of each level in each window of 1 s of its clock, so the rate
/// that a device sets is not the rate of the log. A window starts at the first line of
/// its level after the last window ends. The count of the lines that a window drops
/// goes out as one line `dropped` at that level, with the field `lines`, at the first
/// of the next line of that level, the first [`Log::ended`] after the window, and the
/// drop.
pub(crate) struct Log {
    span: tracing::Span,
    clock: Clock,
    warn: Budget,
    error: Budget,
}

#[derive(Default)]
struct Budget {
    /// The start of the window, or `None` before its first line.
    start: Cell<Option<Monotonic>>,
    written: Cell<u32>,
    dropped: Cell<u64>,
}

impl Log {
    /// Makes a logger whose lines have the parent `span`, and whose windows read
    /// `clock`.
    pub(crate) fn new(span: tracing::Span, clock: Clock) -> Self {
        Self {
            span,
            clock,
            warn: Budget::default(),
            error: Budget::default(),
        }
    }

    /// Writes the count of each window that dropped lines and has ended.
    pub(crate) fn ended(&self) {
        let now = self.clock.now();
        self.ended_at(false, now);
        self.ended_at(true, now);
    }

    fn put(&self, severe: bool, category: &str, text: &str) {
        let now = self.clock.now();
        self.ended_at(severe, now);
        let budget = self.budget(severe);
        if budget.start.get().is_none() {
            budget.start.set(Some(now));
            budget.written.set(0);
        }
        if budget.written.get() == LINES {
            budget.dropped.set(budget.dropped.get() + 1);
            return;
        }
        budget.written.set(budget.written.get() + 1);
        let span = &self.span;
        if severe {
            tracing::error!(target: "open62541", parent: span, category, "{text}");
        } else {
            tracing::warn!(target: "open62541", parent: span, category, "{text}");
        }
    }

    fn ended_at(&self, severe: bool, now: Monotonic) {
        let start = self.budget(severe).start.get();
        if start.is_some_and(|start| now - start >= WINDOW) {
            self.end(severe);
        }
    }

    /// Ends the window of the level, and writes its count when it dropped lines.
    fn end(&self, severe: bool) {
        let budget = self.budget(severe);
        budget.start.set(None);
        let lines = budget.dropped.replace(0);
        if lines == 0 {
            return;
        }
        let span = &self.span;
        if severe {
            tracing::error!(target: "open62541", parent: span, lines, "dropped");
        } else {
            tracing::warn!(target: "open62541", parent: span, lines, "dropped");
        }
    }

    fn budget(&self, severe: bool) -> &Budget {
        if severe { &self.error } else { &self.warn }
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        self.end(false);
        self.end(true);
    }
}

/// Writes the `length` bytes at `line`, a message of the logger of a loop in
/// `category`, through the [`Log`] at `log`: at ERROR when `severe`, else at WARN.
/// Each byte sequence that is not UTF-8 becomes U+FFFD.
///
/// # Safety
///
/// `log` points at a live `Log` that no `&mut` borrows, `category` at a NUL-terminated
/// name in ASCII, and `line` at `length` bytes.
#[unsafe(no_mangle)]
unsafe extern "C" fn connector_opcua_put_line(
    log: *const c_void,
    severe: bool,
    category: *const c_char,
    line: *const c_char,
    length: usize,
) {
    // SAFETY: the caller keeps the log live.
    let log = unsafe { &*log.cast::<Log>() };
    // SAFETY: the caller gives a NUL-terminated name.
    let category = unsafe { CStr::from_ptr(category) };
    let category = category
        .to_str()
        .expect("the shim names a category in ASCII");
    // SAFETY: the caller gives `length` bytes at `line`.
    let text = unsafe { text(line, length) };
    log.put(severe, category, &text);
}

/// Panics with `open62541 ` and the `length` bytes at `line`, as [`text`] reads them,
/// then aborts the process. The panic hook runs once in each build, so the hook of the
/// binary puts the line in its log before the abort.
///
/// # Safety
///
/// `line` points at `length` bytes.
#[unsafe(no_mangle)]
unsafe extern "C" fn connector_opcua_die(line: *const c_char, length: usize) -> ! {
    // SAFETY: the caller gives `length` bytes at `line`.
    let text = unsafe { text(line, length) };
    // In a build that unwinds, a panic out of this frame runs the hook a second time.
    let Err(_) = std::panic::catch_unwind(|| -> ! { panic!("open62541 {text}") });
    std::process::abort()
}

/// Aborts the process as open62541 does at a refused call: it calls
/// `UA_EventLoop_new_POSIX`, which the copy does not build, so the process panics with
/// `open62541 called UA_EventLoop_new_POSIX, which is not built`, the panic hook runs,
/// and the process aborts.
#[cfg(feature = "sim")]
pub fn call_refused() -> ! {
    // SAFETY: it aborts before it reads its argument.
    unsafe { crate::ffi::test::UA_EventLoop_new_POSIX(std::ptr::null()) };
    unreachable!("open62541 returned from a refused call")
}

/// Gives the `length` bytes at `line`, with U+FFFD for each sequence that is not
/// UTF-8.
///
/// # Safety
///
/// `line` points at `length` bytes.
unsafe fn text<'a>(line: *const c_char, length: usize) -> Cow<'a, str> {
    // SAFETY: the caller gives `length` bytes at `line`.
    let bytes = unsafe { std::slice::from_raw_parts(line.cast::<u8>(), length) };
    String::from_utf8_lossy(bytes)
}
