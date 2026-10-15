//! The home end: a hub on a simulated node that serves the session of a program on one
//! `hub::Link`.

use hub::Hub;
use transport::{Address, Session};
use types::time::Span;

use crate::node::env;
use crate::sessions::{self, Home, accept_on};

/// How long a hub with no mesh time waits for it, longer than each test.
const NEVER: Span = Span::from_nanos(3600 * Span::SECOND.nanos());

/// Runs one session as [`sessions::serve_session`] does, on a hub of
/// `hub::testing` with mesh time when `synced`.
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
    let open = async move |node: &sim::node::Node, tasks: &env::tasks::Tasks| {
        (hub(node, tasks, synced).await, ())
    };
    sessions::serve_session(seed, wire, rules, open, async |()| {}, program)
}

/// A hub as [`hub`] gives it, and the session of the first program that dials it,
/// with its link, as [`accept_on`] gives them with `rules`.
pub(crate) async fn accept(
    node: &sim::node::Node,
    tasks: &env::tasks::Tasks,
    synced: bool,
    rules: Option<access::Rules>,
) -> (Hub, Session, hub::Link) {
    let hub = hub(node, tasks, synced).await;
    let (session, link) = accept_on(&hub, node, tasks, rules).await;
    (hub, session, link)
}

/// A hub of `hub::testing` on `node`, with mesh time when `synced`.
async fn hub(node: &sim::node::Node, tasks: &env::tasks::Tasks, synced: bool) -> Hub {
    let seams = env(node, tasks.clone());
    if synced {
        hub::testing::open(seams).await.0
    } else {
        hub::testing::open_unsynced(seams, NEVER).await
    }
}
