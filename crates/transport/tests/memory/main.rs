//! The tests that bound the heap of transport with one count of the bytes it holds.
//! The count covers each thread, so this binary has no test harness. The sim runs on
//! one thread, so the count is exact.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::future::poll_fn;
use std::net::SocketAddr;
use std::pin::pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use block::Block;
use env::clock::Clock;
use sim::Sim;
use transport::stream::Receiver;
use transport::{Address, Class, Code, Error, Transport};
use types::time::Span;

use crate::common::{CLIENT, PORT, SERVER, config, filled, part};

#[path = "../common/mod.rs"]
mod common;
mod dials;
mod ended;
mod failed;
mod held;
mod kept;
mod opened;
mod peers;
mod stretch;

/// The message before the close in [`closed`].
const SHORT: usize = 1000;
/// When the client of [`closed`] closes the session, after its send.
const CLOSE: Span = Span::from_nanos(500_000_000);
/// How long the client of [`closed`] lives after its close: past the server's drop.
const LIVE: Span = Span::from_nanos(2_000_000_000);

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

fn main() {
    let closed = closed();
    held::main(closed);
    kept::main();
    opened::main();
    failed::main(closed);
    stretch::main();
    peers::main();
    dials::main();
    ended::main();
}

/// Polls one `receiver.recv()` each millisecond until it is ready, and calls
/// `on_pending` with the count of polls that gave `Pending` after each one. Gives the
/// read and that count.
pub(crate) async fn next(
    receiver: &mut Receiver,
    clock: &Clock,
    mut on_pending: impl FnMut(usize),
) -> (Result<Option<Block>, Error>, usize) {
    let mut recv = pin!(receiver.recv());
    let mut pending = 0;
    loop {
        if let Poll::Ready(read) =
            poll_fn(|cx| Poll::Ready(recv.as_mut().poll(cx))).await
        {
            return (read, pending);
        }
        pending += 1;
        on_pending(pending);
        clock.sleep(Span::MILLISECOND).await;
    }
}

/// The net heap bytes that the drop of a receiver gives back after a read of a whole
/// message, then a read that gives the close of its session.
fn closed() -> usize {
    let mut sim = Sim::new(sim::Config::default());
    let client = sim.node(sim::node::Config::default());
    let server = sim.node(sim::node::Config::default());
    let address = SocketAddr::new(server.addresses()[0], PORT);
    let out = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&out);
    let shard = env::shards::Config {
        name: "server".into(),
        core: None,
    };
    let own = server.clone();
    let started = server.shards().start(shard, move |tasks| async move {
        let config = config(&own, tasks, SERVER);
        let transport = Transport::new(config, part(&own, PORT)).expect("a transport");
        let session = transport.accept().await.expect("a session");
        let mut receiver = session.accept().await.expect("a stream").receiver;
        let clock = own.clock();
        let (read, _) = next(&mut receiver, &clock, |_| {}).await;
        let len = read.map(|block| block.map(|block| block.len()));
        assert_eq!(len, Ok(Some(SHORT)), "the read before the close");
        let (read, _) = next(&mut receiver, &clock, |_| {}).await;
        assert_eq!(
            read.err(),
            Some(Error::PeerClosed { code: Code(7) }),
            "the close"
        );
        let before = ALLOCATOR.held();
        drop(receiver);
        *kept.lock().expect("not poisoned") =
            Some(before.saturating_sub(ALLOCATOR.held()));
    });
    drop(started.expect("a shard"));
    sim.run_on(&client, move |node, tasks| async move {
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
        sender.send(filled(&pool, SHORT)).await.expect("sent");
        let clock = node.clock();
        clock.sleep(CLOSE).await;
        session.close(Code(7));
        clock.sleep(LIVE).await;
    })
    .expect("the run ends");
    let kept = out.lock().expect("not poisoned").take();
    kept.expect("the server drops its receiver")
}
