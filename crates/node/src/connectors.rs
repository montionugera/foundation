//! Runs the connectors that the spec in use places on this node.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use connector::cancel;
use connector::supervisor::{self, Supervisor};
use spec::connector::Connector;
use spec::definition::Definition;
use types::name::Name;

use crate::scope::Scope;

/// The runs of the connectors that the spec in use places on one node, on one
/// supervisor. Dropped, it drops each run. Each name has one loop, a future that runs
/// its connector, and after each change the connector of that change, until a change
/// removes it. After a run that gave `Config`, the next apply runs the connector again,
/// also one that came before `run` returned. So no two runs of one name overlap, also
/// across a removal and an addition. A first run starts only once no open on the node
/// is pending, so an out connector reads each frame of an in connector of its apply.
pub(crate) struct Runs {
    supervisor: Supervisor,
    opens: Rc<Opens>,
    /// The name of the node in the region.
    node: Name,
    scope: Scope,
    /// The loop of each name, also one whose connector is gone, until the first apply
    /// after it ended.
    loops: BTreeMap<Name, Rc<RefCell<Loop>>>,
}

/// The count of the opens on the node that have not resolved.
struct Opens {
    pending: Cell<usize>,
    /// Cancelled, and replaced, each time `pending` goes to 0.
    settled: RefCell<cancel::Token>,
}

impl Opens {
    /// Counts one open until the guard drops.
    fn begin(self: &Rc<Self>) -> Pending {
        self.pending.set(self.pending.get() + 1);
        Pending(Rc::clone(self))
    }

    /// Resolves once no open is pending.
    async fn settled(&self) {
        if self.pending.get() > 0 {
            let settled = self.settled.borrow().clone();
            settled.wait().await;
        }
    }
}

/// One pending open of [`Opens`].
struct Pending(Rc<Opens>);

impl Drop for Pending {
    fn drop(&mut self) {
        let pending = self.0.pending.get() - 1;
        self.0.pending.set(pending);
        if pending == 0 {
            self.0.settled.replace(cancel::Token::new()).cancel();
        }
    }
}

/// The state that `apply` and the loop of one name share.
struct Loop {
    /// `None` once the spec removed the connector.
    connector: Option<Connector>,
    /// Cancelled when `connector` changes. It cancels the run in progress.
    changed: cancel::Token,
    /// Cancelled at each apply. A run that gave `Config` runs again once the token of
    /// its start is cancelled, also by an apply before `run` returned.
    applied: cancel::Token,
    /// The loop read `None` and ended.
    ended: bool,
}

impl Runs {
    /// Runs no connector of the node `node`, its name in the region, yet. Each loop is
    /// a future on the tasks of `supervisor`, on a supervisor made from it.
    pub(crate) fn new(supervisor: supervisor::Config, node: Name) -> Self {
        Self {
            scope: Scope::new(supervisor.tasks.clone()),
            supervisor: Supervisor::new(supervisor),
            opens: Rc::new(Opens {
                pending: Cell::new(0),
                settled: RefCell::new(cancel::Token::new()),
            }),
            node,
            loops: BTreeMap::new(),
        }
    }

    /// Makes the runs match the connectors of `definitions` on the node. Cancels the
    /// run of each connector that is gone or changed, and starts a run of each one
    /// that is new or changed, or whose last run gave `Config`, after the last run of
    /// its name returned. Each other connector keeps its run. A run starts only once
    /// the open of each connector that this apply adds resolved.
    pub(crate) fn apply(&mut self, definitions: &BTreeMap<Name, Definition>) {
        let wanted: BTreeMap<&Name, &Connector> = definitions
            .iter()
            .filter_map(|(name, definition)| match definition {
                Definition::Connector(connector) if *connector.node() == self.node => {
                    Some((name, connector))
                }
                _ => None,
            })
            .collect();
        self.loops.retain(|name, state| {
            let mut state = state.borrow_mut();
            if state.ended {
                return false;
            }
            state.applied.cancel();
            state.applied = cancel::Token::new();
            let wanted = wanted.get(name).copied();
            if state.connector.as_ref() != wanted {
                state.connector = wanted.cloned();
                state.changed.cancel();
                state.changed = cancel::Token::new();
            }
            true
        });
        for (name, connector) in wanted {
            if self.loops.contains_key(name) {
                continue;
            }
            let state = Rc::new(RefCell::new(Loop {
                connector: Some(connector.clone()),
                changed: cancel::Token::new(),
                applied: cancel::Token::new(),
                ended: false,
            }));
            self.loops.insert(name.clone(), Rc::clone(&state));
            let (supervisor, opens) = (self.supervisor.clone(), Rc::clone(&self.opens));
            // Counted before any loop of this apply polls.
            let mut pending = Some(opens.begin());
            let name = name.clone();
            self.scope.spawn(Box::pin(async move {
                loop {
                    let pending = pending.take().unwrap_or_else(|| opens.begin());
                    let (connector, changed, applied) = {
                        let mut state = state.borrow_mut();
                        let Some(connector) = state.connector.clone() else {
                            state.ended = true;
                            return;
                        };
                        (connector, state.changed.clone(), state.applied.clone())
                    };
                    let kind = connector.kind().as_str();
                    let config = connector.config().document();
                    let opened = supervisor.open(kind, name.clone(), config, &changed);
                    let opened = opened.await;
                    drop(pending);
                    changed.race(opens.settled()).await;
                    match opened.run().await {
                        // The next change of the connector runs it again.
                        Ok(()) => changed.wait().await,
                        // The status holds its class. The fix can be in another
                        // definition, so the next spec change runs it again.
                        Err(connector::kind::Error::Config(_)) => applied.wait().await,
                        Err(error) => {
                            unreachable!(
                                "invariant: `run` gives only `Config`: {error}"
                            )
                        }
                    }
                }
            }));
        }
    }
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests;
