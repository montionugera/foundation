//! A simulated OPC UA server for the tests of other crates: the open62541 test server,
//! with no security, that serves Double variables and records each read that it
//! answers.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::cell::Cell;
use std::collections::BTreeMap;
use std::ffi::CString;
use std::fmt;
use std::future::poll_fn;
use std::iter;
use std::mem::ManuallyDrop;
use std::net::SocketAddr;
use std::pin::pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::Poll;

use env::clock::Clock;
use env::net::{self, Net};
use env::rng::Rng;
use types::quality::Quality;
use types::time::Monotonic;

use crate::connection::Manager;
use crate::node::Node;

/// The sessions and secure channels of a [`Server`].
const SESSIONS: u16 = 4;

/// The Double variables of a [`Server`], each with its value and status, and each read
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

    /// Each read that a server answered, by the text of its node in
    /// [`Variables::new`], in the order of the answers.
    #[must_use]
    pub fn served(&self) -> BTreeMap<String, Vec<Served>> {
        (self.state().variables.iter())
            .map(|variable| (variable.text.clone(), variable.served.clone()))
            .collect()
    }

    /// Answers a read of variable `index` at `time`, and records it when `recorded`.
    fn read(&self, index: usize, time: Monotonic, recorded: bool) -> Served {
        let mut state = self.state();
        let quality = state.quality;
        let variable = &mut state.variables[index];
        let served = Served {
            time,
            value: variable.value,
            quality,
        };
        if recorded {
            variable.served.push(served);
        }
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

/// The node context of a variable: `shim_source` of `server.c`, then the variable.
#[repr(C)]
struct Source {
    read: unsafe extern "C" fn(*mut Source, *mut f64, *mut u32),
    variables: Variables,
    clock: Clock,
    index: usize,
    /// False while the add of the variable reads it to check its type.
    added: Cell<bool>,
}

/// Answers a read of the variable of `source` into `value` and `status`.
///
/// # Safety
///
/// `source` is live, and `value` and `status` are places to write.
unsafe extern "C" fn read(source: *mut Source, value: *mut f64, status: *mut u32) {
    // SAFETY: as the caller ensures.
    let source = unsafe { &*source };
    let (time, added) = (source.clock.now(), source.added.get());
    let served = source.variables.read(source.index, time, added);
    // SAFETY: as the caller ensures.
    unsafe { value.write(served.value) };
    // SAFETY: as the caller ensures.
    unsafe { status.write(served.quality.0) };
}

/// A started server. [`Server::serve`] closes it, and a drop leaks it.
#[must_use = "serve it, since a drop leaks the server"]
pub struct Server {
    manager: ManuallyDrop<Manager>,
    started: crate::server::Server,
    /// Each lives until the server is deleted.
    sources: Vec<NonNull<Source>>,
    local: SocketAddr,
}

impl Server {
    /// Listens on `local`, and starts a server of `variables` with room for 4
    /// sessions. Each namespace from 2 to the highest of `variables` has the URI
    /// `urn:foundation:sim:{index}`. Make it, serve it, and drop it on one thread that
    /// `env` started.
    ///
    /// # Errors
    ///
    /// The error of the listen.
    ///
    /// # Panics
    ///
    /// If open62541 refuses the server, a namespace, or a variable.
    pub fn listen(
        clock: Clock,
        net: Net,
        local: SocketAddr,
        rng: &mut Rng,
        variables: &Variables,
    ) -> Result<Self, net::Error> {
        let sessions = u32::from(SESSIONS);
        let manager =
            Manager::listening(Clock::clone(&clock), net, local, sessions, rng)?;
        Ok(Self::start(
            clock,
            ManuallyDrop::new(manager),
            local,
            variables,
        ))
    }

    /// Starts the server of [`Server::listen`] on `manager`.
    fn start(
        clock: Clock,
        manager: ManuallyDrop<Manager>,
        local: SocketAddr,
        variables: &Variables,
    ) -> Self {
        let port = (manager.port()).expect("invariant: a listening manager has a port");
        // SAFETY: `serve` deletes the server before the manager drops, and a drop leaks
        // both.
        let started = unsafe { crate::server::Server::start(&manager, c"", SESSIONS) };
        let nodes: Vec<Node> = (variables.state().variables.iter())
            .map(|variable| variable.node.clone())
            .collect();
        let top = nodes.iter().map(|node| node.namespace).max().unwrap_or(0);
        for namespace in 2..=top {
            let name = CString::new(format!("urn:foundation:sim:{namespace}"))
                .expect("invariant: a name with no NUL");
            // SAFETY: the server lives, and it copies the name.
            let added = unsafe {
                crate::ffi::test::UA_Server_addNamespace(started.raw(), name.as_ptr())
            };
            assert_eq!(added, namespace, "open62541 refused a namespace");
        }
        let clocks = iter::repeat_n(clock, nodes.len());
        let sources = (nodes.iter().zip(clocks).enumerate())
            .map(|(index, (node, clock))| {
                let source = NonNull::from(Box::leak(Box::new(Source {
                    read,
                    variables: Variables::clone(variables),
                    clock,
                    index,
                    added: Cell::new(false),
                })));
                // SAFETY: a `Source` starts with its read function, and `serve` frees
                // it after the delete.
                unsafe { started.add_source(node, source.as_ptr().cast()) };
                // SAFETY: it lives, and the server only reads it on this thread.
                unsafe { source.as_ref() }.added.set(true);
                source
            })
            .collect();
        Self {
            manager,
            started,
            sources,
            local: SocketAddr::new(local.ip(), port),
        }
    }

    /// The address that it listens on.
    #[must_use]
    pub fn local(&self) -> SocketAddr {
        self.local
    }

    /// Serves until `until` completes, then closes the server.
    ///
    /// # Panics
    ///
    /// If open62541 refuses a step of the close.
    pub async fn serve(self, until: impl Future<Output = ()>) {
        let Self {
            manager,
            started,
            sources,
            ..
        } = self;
        {
            let mut drive = pin!(manager.drive(|_| {
                manager.events().run();
                Poll::<()>::Pending
            }));
            let mut until = pin!(until);
            poll_fn(|cx| {
                assert!(drive.as_mut().poll(cx).is_pending(), "a drive never ends");
                until.as_mut().poll(cx)
            })
            .await;
        }
        started.close(&manager).await;
        drop(ManuallyDrop::into_inner(manager));
        for source in sources {
            // SAFETY: the server that read it is deleted, and nothing else holds it.
            drop(unsafe { Box::from_raw(source.as_ptr()) });
        }
    }
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
