//! The tests that bound the allocations of transport with one count of them. The
//! count covers each thread, so this binary has no test harness. The sim runs on one
//! thread, so the count is exact.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::net::SocketAddr;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use block::{Block, Pool};
use sim::Sim;
use sim::node::Node;
use transport::{Address, Class, Transport};
use types::time::Span;

use crate::common::{CLIENT, SERVER, config, part};

mod chunks;
#[path = "../common/mod.rs"]
mod common;
mod parts;
mod polls;
mod sends;
mod session;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    polls::main();
    chunks::main();
    parts::main();
    sends::main();
}

/// The bytes of the message after the long one on a stream.
pub(crate) const SHORT: usize = 1000;
/// When the server reads after it accepts the stream, once the messages are in.
pub(crate) const READ: Span = Span::from_nanos(1_000_000_000);
/// How long the client lives after its sends: past the server's reads.
pub(crate) const LIVE: Span = Span::from_nanos(2_000_000_000);

/// Runs `sim` with a client on `node` that dials the server at `address`, sends on
/// one stream each message that `messages` makes from its pool, then lives [`LIVE`].
pub(crate) fn send(
    sim: &mut Sim,
    node: &Node,
    address: SocketAddr,
    messages: impl FnOnce(&Pool) -> Vec<Block> + Send + 'static,
) {
    sim.run_on(node, move |node, tasks| async move {
        let config = config(&node, tasks, CLIENT);
        let pool = Rc::clone(&config.pool);
        let transport = Transport::new(config, part(&node, 0)).expect("a transport");
        let session = transport
            .dial(SERVER.public(), &[Address::Udp(address)])
            .await
            .expect("a session");
        let mut sender = session
            .open_sender(Class::Complete)
            .await
            .expect("a stream");
        for message in messages(&pool) {
            sender.send(message).await.expect("sent");
        }
        node.clock().sleep(LIVE).await;
    })
    .expect("the run ends");
}

/// One poll of `read`, and the allocations it makes.
pub(crate) fn once<T>(read: Pin<&mut impl Future<Output = T>>) -> (Poll<T>, u64) {
    let mut cx = Context::from_waker(Waker::noop());
    ALLOCATOR.count(|| read.poll(&mut cx))
}
