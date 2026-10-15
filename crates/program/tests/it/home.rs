//! The home end: a hub on a simulated node that serves the session of a program on one
//! `hub::Link`.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use hub::{Hub, serve};
use transport::{Address, Session};
use types::ed25519::PrivateKey;
use types::time::Span;

use crate::node::env;
use crate::sessions::{Got, accept_on, recorded, run_program_on, serve_each};

/// A key that the spec does not list.
pub(crate) const OTHER: PrivateKey = PrivateKey([4; 32]);
/// How long the program waits for what must not come.
pub(crate) const QUIET: Span = Span::from_nanos(200_000_000);
/// How long a hub with no mesh time waits for it, longer than each test.
const NEVER: Span = Span::from_nanos(3600 * Span::SECOND.nanos());

/// What the home saw: what `serve` gave for each stream, in the order they ended, and
/// how the session ended.
#[derive(Debug)]
pub(crate) struct Home {
    pub(crate) served: Vec<Result<Got, serve::Error>>,
    pub(crate) closed: transport::Error,
}

/// Runs one session: the home's node makes a hub, with mesh time when `synced` and
/// `rules` set unless `None`, and serves each hub stream of the session on one
/// `hub::Link`, as `node` does. `program` runs on the program's node with the home's
/// address, and dials it itself.
pub(crate) fn serve_session<G, P>(
    seed: u64,
    synced: bool,
    rules: Option<access::Rules>,
    program: G,
) -> Home
where
    G: FnOnce(sim::node::Node, env::tasks::Tasks, Address) -> P + Send + 'static,
    P: Future<Output = ()> + 'static,
{
    serve_session_on(seed, sim::link::Config::default(), synced, rules, program)
}

/// As [`serve_session`], on `wire`.
pub(crate) fn serve_session_on<G, P>(
    seed: u64,
    wire: sim::link::Config,
    synced: bool,
    rules: Option<access::Rules>,
    program: G,
) -> Home
where
    G: FnOnce(sim::node::Node, env::tasks::Tasks, Address) -> P + Send + 'static,
    P: Future<Output = ()> + 'static,
{
    let served = Arc::new(Mutex::new(Vec::new()));
    let closed = Arc::new(Mutex::new(None));
    let (kept, ended) = (Arc::clone(&served), Arc::clone(&closed));
    let home = move |node: sim::node::Node, tasks: env::tasks::Tasks| async move {
        let (hub, session, link) = accept(&node, &tasks, synced, rules).await;
        let accepted = Rc::new(Cell::new(0));
        serve_each(&session, &link, &tasks, &node.clock(), &kept, &accepted).await;
        *ended.lock().expect("not poisoned") = Some(session.closed().await);
        recorded(&node, &kept, accepted.get()).await;
        drop((link, hub));
    };
    run_program_on(seed, wire, home, program);
    let served = std::mem::take(&mut *served.lock().expect("not poisoned"));
    let closed = closed.lock().expect("not poisoned").take();
    Home {
        served,
        closed: closed.expect("the session closed"),
    }
}

/// Makes the error of the wall clock of `node` unknown, and 10 ms one minute later.
pub(crate) fn shrink_wall_error(node: &sim::node::Node, tasks: &env::tasks::Tasks) {
    node.set_wall_error(None);
    let shrink = node.clone();
    tasks.spawn(async move {
        shrink.clock().sleep(Span::MINUTE).await;
        shrink.set_wall_error(Some(Span::from_nanos(10_000_000)));
    });
}

/// A hub on `node`, with mesh time when `synced` and `rules` set unless `None`, and
/// the session of the first program that dials it, with its link.
pub(crate) async fn accept(
    node: &sim::node::Node,
    tasks: &env::tasks::Tasks,
    synced: bool,
    rules: Option<access::Rules>,
) -> (Hub, Session, hub::Link) {
    let seams = env(node, tasks.clone());
    let hub = if synced {
        hub::testing::open(seams).await.0
    } else {
        hub::testing::open_unsynced(seams, NEVER).await
    };
    let (session, link) = accept_on(&hub, node, tasks, rules).await;
    (hub, session, link)
}
