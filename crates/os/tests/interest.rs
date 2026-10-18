//! A callsite that a thread with no log meets first, on `os`. `tracing` caches the
//! interest of each callsite for the process, so this runs in a test binary of its own,
//! with this one test only.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "../../env/tests/common/log.rs"]
#[expect(
    dead_code,
    unused_imports,
    unused_macros,
    reason = "this binary uses only the log"
)]
mod recorded;

use recorded::Log;

fn seen() {
    tracing::info!("seen");
}

/// A thread with no log meets a callsite first, while the one live dispatch is the log
/// of another thread: that thread gets its event.
#[test]
fn a_log_gets_an_event_of_a_callsite_that_a_thread_with_no_log_met_first() {
    let threads = os::threads().unwrap();
    let log = Log::default();
    let logged = threads.logged(log.dispatch());
    let unlogged = threads.start("unlogged", || async { seen() });
    unlogged.unwrap().join().unwrap();
    logged
        .start("logged", || async { seen() })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(log.messages(), ["seen"]);
}
