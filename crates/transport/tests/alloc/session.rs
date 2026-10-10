//! A session of a client and a server on two sim nodes, for the allocation tests.

use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use block::{Block, Heap, Pool};
use env::clock::Clock;
use sim::Sim;
use sim::node::Node;
use transport::stream::{Part, Sender};
use transport::{Address, Code, Config, Error, Session, Transport};
use types::ed25519::PrivateKey;
use types::time::Span;

use crate::common::{CLIENT, PORT, SERVER, part};

/// Long enough for the peer to read and acknowledge a message.
pub(crate) const PAUSE: Span = Span::from_nanos(100_000_000);
const CLOSED: Error = Error::PeerClosed { code: Code(0) };
/// The sends of each kind before the counted ones.
pub(crate) const WARMUP: usize = 8;
pub(crate) const ROUNDS: usize = 8;

/// The limits of a transport: its largest message and its window.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) message: usize,
    pub(crate) window: usize,
}

pub(crate) const WIDE: Limits = Limits {
    message: 1 << 17,
    window: 1 << 20,
};
/// The smallest window for the largest message: twice its size.
pub(crate) const NARROW: Limits = Limits {
    message: 16_000,
    window: 32_000,
};

/// Runs `client` on a session of a client and a server with `limits`, then closes the
/// session.
pub(crate) fn run(
    limits: Limits,
    client: impl AsyncFnOnce(Session, Rc<Pool>, Clock) + Send + 'static,
) {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let server = sim.node(sim::node::Config::default());
    let at = SocketAddr::new(server.addresses()[0], PORT);
    serve(&server, limits);
    sim.run_on(&node, move |node, tasks| async move {
        let config = config(&node, tasks, CLIENT, limits);
        let pool = Rc::clone(&config.pool);
        let transport = Transport::new(config, part(&node, 0)).expect("a transport");
        let session = transport
            .dial(SERVER.public(), &[Address::Udp(at)])
            .await
            .expect("a session");
        let clock = node.clock();
        client(session.clone(), pool, clock.clone()).await;
        clock.sleep(PAUSE).await;
        session.close(Code(0));
        clock.sleep(Span::MILLISECOND).await;
    })
    .expect("the test runs");
}

/// Polls a send of `parts` of `block` once, or of all of it when `parts` is empty.
///
/// # Panics
///
/// When the send waits or fails.
pub(crate) fn poll(sender: &mut Sender, block: Block, parts: &[Part]) {
    let mut cx = Context::from_waker(Waker::noop());
    let polled = if parts.is_empty() {
        pin!(sender.send(block)).poll(&mut cx)
    } else {
        pin!(sender.send_parts(block, parts)).poll(&mut cx)
    };
    match polled {
        Poll::Ready(sent) => sent.expect("the send goes"),
        Poll::Pending => panic!("a send waited on its first poll"),
    }
}

fn serve(node: &Node, limits: Limits) {
    let own = node.clone();
    let shard = env::shards::Config {
        name: "server".into(),
        core: None,
    };
    let started = node.shards().start(shard, move |tasks| async move {
        let config = config(&own, tasks.clone(), SERVER, limits);
        let transport = Transport::new(config, part(&own, PORT)).expect("a transport");
        let session = transport.accept().await.expect("a session");
        let mut receiver = session.accept().await.expect("a stream").receiver;
        loop {
            match receiver.recv().await {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(error) => panic!("the server failed to read: {error}"),
            }
        }
        assert_eq!(session.closed().await, CLOSED, "the client closes");
    });
    drop(started.expect("a shard"));
}

/// [`crate::common::config`] with `limits` and a pool of 4 MiB.
fn config(
    node: &Node,
    tasks: env::tasks::Tasks,
    key: PrivateKey,
    limits: Limits,
) -> Config {
    let pool = block::Config::new(1 << 22).expect("the budget fits");
    let memory = Heap::new(pool.reservation());
    Config {
        message_bytes_max: NonZeroUsize::new(limits.message).expect("not zero"),
        window_bytes: limits.window,
        pool: Rc::new(Pool::new(pool, memory)),
        ..crate::common::config(node, tasks, key)
    }
}
