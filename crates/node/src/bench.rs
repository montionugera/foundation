//! `Scope` and the log dispatch, for the bench targets only. Not a stable surface.

use std::cell::Cell;
use std::rc::Rc;

use env::tasks::Task;

/// Futures that drop together, as `node` runs each session and stream. Dropped, it
/// drops each future that has not completed and wakes its task, which then ends.
#[expect(
    missing_debug_implementations,
    reason = "a bench surface over a scope with no `Debug`"
)]
pub struct Scope {
    scope: crate::scope::Scope,
    spawned: Rc<Cell<Option<Task>>>,
}

impl Scope {
    /// An empty scope whose tasks the caller polls by hand.
    #[must_use]
    pub fn new() -> Self {
        let spawned = Rc::default();
        let driver = Spawned(Rc::clone(&spawned));
        Self {
            scope: crate::scope::Scope::new(env::tasks::Tasks::new(driver)),
            spawned,
        }
    }

    /// Spawns `future` in the scope and gives the task that polls it.
    #[must_use = "the future runs only when the caller polls the task"]
    pub fn spawn(&mut self, future: Task) -> Task {
        self.scope.spawn(future);
        self.spawned
            .take()
            .expect("invariant: a spawn of the scope spawns one task")
    }
}

impl Default for Scope {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps the task of the last spawn for [`Scope::spawn`] to give.
struct Spawned(Rc<Cell<Option<Task>>>);

impl env::tasks::Driver for Spawned {
    fn spawn(&self, task: Task) {
        self.0.set(Some(task));
    }
}

/// The log of a node, with its thread `log` running until the drop.
#[derive(Debug)]
pub struct Log {
    flush: crate::Flush,
    _hold: crate::log::Hold,
}

impl Log {
    /// The dispatch that a node sets for each shard.
    #[must_use]
    pub fn dispatch(&self) -> &tracing::Dispatch {
        &self.flush.dispatch
    }
}

/// The log that a node opens: each event of its level or above, stamped with the time
/// of `wall` and the mesh time of `mesh`, and written on a thread of `threads`.
///
/// # Panics
///
/// When the thread `log` cannot start.
#[must_use]
pub fn log(
    log: crate::Log,
    threads: &env::threads::Threads,
    wall: env::wall::Wall,
    mesh: clock::Reader,
) -> Log {
    let flush = crate::log::open(log, threads, wall, mesh);
    let hold = flush.hold().expect("the thread log starts");
    Log { flush, _hold: hold }
}
