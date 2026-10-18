//! The lines of the logger of each loop and the aborts of `shim.c`, which C gives to
//! Rust: each line becomes an event of `tracing`, and each abort a panic.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::borrow::Cow;
use std::ffi::{CStr, c_char, c_void};

/// Writes the `length` bytes at `line`, a message of the logger of a loop in
/// `category`, as one event of target `open62541` whose parent is `span`: ERROR when
/// `severe`, else WARN. Each byte sequence that is not UTF-8 becomes U+FFFD.
///
/// # Safety
///
/// `span` points at a live `tracing::Span`, `category` at a NUL-terminated name in
/// ASCII, and `line` at `length` bytes.
#[unsafe(no_mangle)]
unsafe extern "C" fn connector_opcua_put_line(
    span: *const c_void,
    severe: bool,
    category: *const c_char,
    line: *const c_char,
    length: usize,
) {
    // SAFETY: the caller keeps the span live.
    let span = unsafe { &*span.cast::<tracing::Span>() };
    // SAFETY: the caller gives a NUL-terminated name.
    let category = unsafe { CStr::from_ptr(category) };
    let category = category
        .to_str()
        .expect("the shim names a category in ASCII");
    // SAFETY: the caller gives `length` bytes at `line`.
    let text = unsafe { text(line, length) };
    if severe {
        tracing::error!(target: "open62541", parent: span, category, "{text}");
    } else {
        tracing::warn!(target: "open62541", parent: span, category, "{text}");
    }
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
