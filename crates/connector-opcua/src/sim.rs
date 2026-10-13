//! A simulated OPC UA server for the tests of other crates: the open62541 test server,
//! with no security, that serves Double variables and records each read that it
//! answers.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::sync::{Arc, Mutex, MutexGuard};

use env::clock::Clock;
use env::net::{Listener, Net};
use env::rng::Rng;
use types::quality::Quality;
use types::time::{Monotonic, Stamp};

use crate::connection::Manager;
use crate::event;
use crate::node::Node;
use crate::server::Server;

/// The sessions and secure channels of [`serve`].
const SESSIONS: u16 = 4;

/// The Double variables of [`serve`], each with its value and status, and each read
/// that the server answered. Clones share them.
#[derive(Clone, Debug)]
pub struct Variables(Arc<Mutex<State>>);

#[derive(Debug)]
struct State {
    variables: Vec<Variable>,
    /// The status of each later read.
    quality: Quality,
}

#[derive(Debug)]
struct Variable {
    /// The text of the node in [`Variables::new`].
    text: String,
    node: Node,
    value: f64,
    served: Vec<Served>,
}

impl Variables {
    /// The variables at the node ids `nodes`, each with the value 0 and the status
    /// Good.
    ///
    /// # Panics
    ///
    /// If a text of `nodes` is not a node id, or two name the same node.
    #[must_use]
    pub fn new(nodes: &[&str]) -> Self {
        let mut variables: Vec<Variable> = Vec::with_capacity(nodes.len());
        for &text in nodes {
            let node: Node = (text.parse())
                .unwrap_or_else(|error| panic!("{text:?} is not a node id: {error}"));
            if let Some(twin) = variables.iter().find(|variable| variable.node == node)
            {
                panic!("{text:?} and {:?} name the same node", twin.text);
            }
            variables.push(Variable {
                text: text.into(),
                node,
                value: 0.0,
                served: Vec::new(),
            });
        }
        Self(Arc::new(Mutex::new(State {
            variables,
            quality: Quality::GOOD,
        })))
    }

    /// Adds 1 to the value of each variable, and gives each later read the status
    /// `quality`.
    pub fn change(&self, quality: Quality) {
        let mut state = self.state();
        for variable in &mut state.variables {
            variable.value += 1.0;
        }
        state.quality = quality;
    }

    /// Sets the value of the variable of `text` in [`Variables::new`].
    ///
    /// # Panics
    ///
    /// If no variable has `text`.
    #[cfg(test)]
    pub(crate) fn set(&self, text: &str, value: f64) {
        let mut state = self.state();
        let variable = state.variables.iter_mut().find(|v| v.text == text);
        variable
            .unwrap_or_else(|| panic!("no variable {text:?}"))
            .value = value;
    }

    /// Each read that a server answered, by the text of its node in
    /// [`Variables::new`], in the order of the answers.
    #[must_use]
    pub fn served(&self) -> BTreeMap<String, Vec<Served>> {
        (self.state().variables.iter())
            .map(|variable| (variable.text.clone(), variable.served.clone()))
            .collect()
    }

    /// Answers a read of variable `index` at `time`, and records it.
    fn read(&self, index: usize, time: Monotonic) -> Served {
        let mut state = self.state();
        let quality = state.quality;
        let variable = &mut state.variables[index];
        let served = Served {
            time,
            value: variable.value,
            quality,
        };
        variable.served.push(served);
        served
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().expect("no holder of the variables panics")
    }
}

/// A read that a server answered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Served {
    /// The time of the answer, on the clock of the server.
    pub time: Monotonic,
    /// The value of the answer.
    pub value: f64,
    /// The status of the answer.
    pub quality: Quality,
}

/// Serves `variables` on `listener` with room for 4 sessions until `until` completes,
/// then closes the server. It adds each namespace from 2 to the highest of
/// `variables`. Each stream of the server has the options of `listener`. `wall` gives
/// the wall time of the server, which a client compares with the time of each channel
/// token: give the wall time of the client's node. Call it on a thread that `env`
/// started. A drop of the future while it waits, after a poll that gives `Pending`,
/// leaks the server and can keep the port of `listener` bound.
///
/// # Panics
///
/// If open62541 refuses the server, a namespace, a variable, or a step of the close.
/// If a variable has the number 0, which open62541 would serve at a free number.
///
/// # Aborts
///
/// When open62541 cannot allocate the loop or the connection manager: the process
/// stops as the Rust allocator does when it has no memory.
pub async fn serve(
    clock: Clock,
    wall: impl Fn() -> Stamp + 'static,
    net: Net,
    listener: Listener,
    rng: &mut Rng,
    variables: &Variables,
    until: impl Future<Output = ()>,
) {
    let time = event::Time {
        clock: Clock::clone(&clock),
        wall: Box::new(wall),
    };
    let manager = Manager::accepting(time, net, listener, rng);
    let nodes: Vec<Node> = (variables.state().variables.iter())
        .map(|variable| variable.node.clone())
        .collect();
    let add = |server: &mut Server| {
        let top = nodes.iter().map(|node| node.namespace).max().unwrap_or(0);
        for namespace in 2..=top {
            let uri = CString::new(format!("urn:foundation:sim:{namespace}"))
                .expect("invariant: a URI with no NUL");
            let added = server.add_namespace(&uri);
            assert_eq!(added, namespace, "open62541 refused a namespace");
        }
        for (index, node) in nodes.iter().enumerate() {
            let (variables, clock) =
                (Variables::clone(variables), Clock::clone(&clock));
            server.add_source(node, move || {
                let read = variables.read(index, clock.now());
                (read.value, read.quality)
            });
        }
    };
    Server::serve(manager, SESSIONS, add, until).await;
}

#[cfg(test)]
mod tests;
