//! A callsite that a thread with no log meets first. `tracing` caches the interest of
//! each callsite for the process, so this runs in a test binary of its own, with this
//! one test only.

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
use types::time::Span;

fn seen() {
    tracing::info!("seen");
}

/// A thread with no log meets a callsite first, then a thread with a log meets it:
/// the log gets its event.
#[test]
fn a_log_gets_an_event_of_a_callsite_that_a_thread_with_no_log_met_first() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let log = Log::default();
    let started = node.threads().start("unlogged", || async { seen() });
    drop(started.expect("the thread starts"));
    let clock = node.clock();
    let started =
        (node.threads().logged(log.dispatch())).start("logged", move || async move {
            clock.sleep(Span::SECOND).await;
            seen();
        });
    drop(started.expect("the thread starts"));
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(log.messages(), ["seen"]);
}
