//! A stream gives back its parts list when it ends: at a peer stop, at a cancel that
//! resets it, and at the close of its connection by either side, while the `Sender`
//! stays.

use std::mem;
use std::net::SocketAddr;
use std::ops::Range;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use block::Pool;
use sim::Sim;
use sim::node::Node;
use transport::stream::{Part, Sender};
use transport::{Address, Class, Code, Error, Transport};
use types::time::Span;

use crate::ALLOCATOR;
use crate::common::{CLIENT, PORT, SERVER, config, filled, part};

/// Long enough for the peer to read a message.
const PAUSE: Span = Span::from_nanos(100_000_000);
/// When the server stops the stream or closes the session, after it reads the first
/// message: after the client cuts its send, and before the client looks again.
const ACT: Span = Span::from_nanos(200_000_000);
/// The parts of the message that the window cuts.
const PARTS: usize = 150;
/// The bytes of each part: one over the longest chunk that noq-proto copies, so that
/// each part stays apart in the list.
const RUN: usize = 1453;
/// The distance from the start of a part to the start of the next.
const STRIDE: usize = 1500;
/// The bytes of the last message that fills the window, after three of 256 KiB.
const FILL: usize = 250_000;

/// How the stream ends.
#[derive(Clone, Copy, Debug)]
enum End {
    Stop,
    Cancel,
    Close,
    PeerClose,
}

pub(crate) fn main() {
    for end in [End::Stop, End::Cancel, End::Close, End::PeerClose] {
        let [sent, sent_after] = run(end, false);
        let [parted, parted_after] = run(end, true);
        // The heap also changes for other causes in the end, the same in each run.
        let list = PARTS * mem::size_of::<Range<usize>>();
        assert!(
            parted + sent_after >= sent + parted_after + list,
            "{end:?}: the heap goes from {parted} to {parted_after} bytes after a \
             message of {PARTS} parts, and from {sent} to {sent_after} after a send"
        );
    }
}

/// The heap bytes that the client holds just before and just after its stream ends
/// in the way of `end`, while the window cuts a message of [`PARTS`] parts when
/// `parted`, else of one block of the same bytes.
fn run(end: End, parted: bool) -> [usize; 2] {
    let mut sim = Sim::new(sim::Config::default());
    let client = sim.node(sim::node::Config::default());
    let server = sim.node(sim::node::Config::default());
    let at = SocketAddr::new(server.addresses()[0], PORT);
    serve(&server, end);
    sim.run_on(&client, move |node, tasks| async move {
        let config = config(&node, tasks, CLIENT);
        let pool = Rc::clone(&config.pool);
        let transport = Transport::new(config, part(&node, 0)).expect("a transport");
        let session = transport
            .dial(SERVER.public(), &[Address::Udp(at)])
            .await
            .expect("a session");
        let mut sender = session
            .open_sender(Class::Complete)
            .await
            .expect("a stream");
        let clock = node.clock();
        sender.send(filled(&pool, 1)).await.expect("sent");
        clock.sleep(PAUSE).await;
        fill(&mut sender, &pool);
        let parts = parts();
        let mut send: Pin<Box<dyn Future<Output = Result<(), Error>>>> = if parted {
            let block = filled(&pool, PARTS * STRIDE);
            Box::pin(sender.send_parts(block, &parts))
        } else {
            let block = filled(&pool, PARTS * RUN);
            Box::pin(sender.send(block))
        };
        let mut cx = Context::from_waker(Waker::noop());
        assert!(
            send.as_mut().poll(&mut cx).is_pending(),
            "the window cuts the send"
        );
        let before = ALLOCATOR.held();
        let mut sending = Some(send);
        let expected = match end {
            End::Stop => {
                clock.sleep(ACT).await;
                Error::Stopped { code: Code(7) }
            }
            End::PeerClose => {
                let closed = Error::PeerClosed { code: Code(9) };
                assert_eq!(session.closed().await, closed, "the server closes");
                closed
            }
            End::Close => {
                session.close(Code(9));
                Error::Closed { code: Code(9) }
            }
            End::Cancel => {
                sending = None;
                Error::Reset { code: Code(0) }
            }
        };
        let after = ALLOCATOR.held();
        if let Some(sent) = sending.map(|mut send| send.as_mut().poll(&mut cx)) {
            assert_eq!(sent, Poll::Ready(Err(expected)), "{end:?}: the cut send");
        } else {
            let written = sender.try_send(filled(&pool, 1)).map(|_| ());
            assert_eq!(written, Err(expected), "{end:?}: a send after the cancel");
        }
        session.close(Code(0));
        clock.sleep(Span::MILLISECOND).await;
        [before, after]
    })
    .expect("the run ends")
}

/// The [`PARTS`] parts of [`RUN`] bytes, [`STRIDE`] bytes apart.
fn parts() -> Vec<Part> {
    (0..PARTS)
        .map(|at| Part {
            range: at * STRIDE..at * STRIDE + RUN,
            zeros: 0,
        })
        .collect()
}

/// Sends messages that leave the window room for a few parts of the next one.
fn fill(sender: &mut Sender, pool: &Pool) {
    let large = filled(pool, 1 << 18);
    for block in [large.clone(), large.clone(), large, filled(pool, FILL)] {
        let left = sender.try_send(block).expect("sent");
        assert!(left.is_none(), "the window takes the fill");
    }
}

/// Starts the server on `node`. It reads the first message and no more, then, at
/// [`ACT`], stops the stream or closes the session as `end` says.
fn serve(node: &Node, end: End) {
    let own = node.clone();
    let shard = env::shards::Config {
        name: "server".into(),
        core: None,
    };
    let started = node.shards().start(shard, move |tasks| async move {
        let config = config(&own, tasks, SERVER);
        let transport = Transport::new(config, part(&own, PORT)).expect("a transport");
        let session = transport.accept().await.expect("a session");
        let mut receiver = session.accept().await.expect("a stream").receiver;
        let first = receiver.recv().await.expect("a message");
        assert!(first.is_some(), "the first message");
        let closed = match end {
            End::Stop => {
                own.clock().sleep(ACT).await;
                receiver.stop(Code(7));
                Error::PeerClosed { code: Code(0) }
            }
            End::PeerClose => {
                own.clock().sleep(ACT).await;
                session.close(Code(9));
                drop(receiver);
                Error::Closed { code: Code(9) }
            }
            End::Cancel => Error::PeerClosed { code: Code(0) },
            End::Close => Error::PeerClosed { code: Code(9) },
        };
        assert_eq!(
            session.closed().await,
            closed,
            "{end:?}: the end of the session"
        );
    });
    drop(started.expect("a shard"));
}
