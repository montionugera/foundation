//! A thread outside the sim meets a callsite while the one live dispatch is its own
//! log, and its interest call is still in progress when a sim with a log starts.
//! `tracing` caches the interest of each callsite for the process, so this runs in a
//! test binary of its own, with this one test only.

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

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use recorded::Log;
use tracing::span::{Attributes, Id, Record};
use tracing::subscriber::Interest;
use tracing::{Event, Metadata, Subscriber};
use types::time::Span;

fn seen() {
    tracing::info!("seen");
}

/// A log that wants no event. Its first interest call takes 200 ms, as a thread that
/// the OS takes off its core, or a filter that waits on its lock, does.
struct Slow {
    /// Tells the test that the first interest call started.
    met: Mutex<Option<mpsc::Sender<()>>>,
    slowed: AtomicBool,
}

impl Subscriber for Slow {
    #[expect(clippy::disallowed_methods, reason = "the delay is of an OS thread")]
    fn register_callsite(&self, meta: &'static Metadata<'static>) -> Interest {
        if meta.target() == module_path!() && !self.slowed.swap(true, Ordering::SeqCst)
        {
            if let Some(met) = self.met.lock().unwrap().take() {
                met.send(()).unwrap();
            }
            thread::sleep(Duration::from_millis(200));
        }
        Interest::never()
    }

    fn enabled(&self, _: &Metadata<'_>) -> bool {
        false
    }

    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _: &Id, _: &Record<'_>) {}

    fn record_follows_from(&self, _: &Id, _: &Id) {}

    fn event(&self, _: &Event<'_>) {}

    fn enter(&self, _: &Id) {}

    fn exit(&self, _: &Id) {}
}

/// A thread outside the sim meets a callsite first, and a sim thread with a log meets
/// it later: the log gets its event.
#[test]
#[expect(clippy::disallowed_methods, reason = "the race is with an OS thread")]
fn a_log_gets_an_event_of_a_callsite_that_another_thread_meets_as_the_sim_starts() {
    let (met, waits) = mpsc::channel();
    let slow = tracing::Dispatch::new(Slow {
        met: Mutex::new(Some(met)),
        slowed: AtomicBool::new(false),
    });
    let other = {
        let slow = slow.clone();
        thread::spawn(move || tracing::dispatcher::with_default(&slow, seen))
    };
    waits.recv().unwrap();
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let log = Log::default();
    let clock = node.clock();
    let started =
        (node.threads().logged(log.dispatch())).start("logged", move || async move {
            clock.sleep(Span::SECOND).await;
            seen();
        });
    drop(started.expect("the thread starts"));
    other.join().unwrap();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(log.messages(), ["seen"]);
}
