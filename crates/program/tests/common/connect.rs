//! The session of the program in the tests of `program`.

use std::rc::Rc;

use program::{Config, Error, Session};
use transport::Address;
use types::ed25519::PrivateKey;

use crate::agent::{SUBJECT, name};
use crate::net::{HOME, NODE, own_pool, public_key};

/// Connects to the home at `at` from `node` as [`SUBJECT`], signing with `key`, with a
/// session pool that holds a body at the cap while the transport sends it.
pub(crate) async fn connect(
    node: &sim::node::Node,
    tasks: env::tasks::Tasks,
    at: Address,
    key: PrivateKey,
) -> Result<Session, Error> {
    let config = block::Config::new(2 << 24).expect("the budget fits");
    let pool = block::Pool::new(config.clone(), block::Heap::new(config.reservation()));
    connect_with(node, tasks, at, key, Rc::new(pool)).await
}

/// As [`connect`], where the session sends from `pool`, and the transport from a pool
/// of its own.
pub(crate) async fn connect_with(
    node: &sim::node::Node,
    tasks: env::tasks::Tasks,
    at: Address,
    key: PrivateKey,
    pool: Rc<block::Pool>,
) -> Result<Session, Error> {
    let config = transport::client::Config {
        net: node.net(),
        clock: node.clock(),
        entropy: node.entropy(),
        tasks: tasks.clone(),
        pool: own_pool(),
    };
    let transport = transport::Client::new(config).expect("a client");
    let config = Config {
        via: NODE,
        node: public_key(&HOME),
        addresses: vec![at],
        subject: name(SUBJECT),
        key,
        clock: node.clock(),
        entropy: node.entropy(),
        tasks,
        pool,
    };
    Session::connect(&transport, config).await
}
