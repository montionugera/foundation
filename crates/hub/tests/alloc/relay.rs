//! A client read against a HUB WIRE session of the same channels at one home.

use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use env::tasks::Tasks;
use hub::{Served, serve, writer};
use transport::stream::Receiver;
use transport::{Address, Class};
use types::authority::Authority;
use types::time::Span;
use wire::Protocol;
use wire::hub::FromHome;
use wire::hub::client::Mode;

use crate::agent::{Agent, SUBJECT};
use crate::{ALLOCATOR, COUNTED, WARM, common, net, peer, sessions};

/// The longest wait for the readers to take a frame.
const TAKE: Span = Span::SECOND;
/// The time from a write to the poll of its frame. Longer than the 25 ms for which a
/// QUIC peer may hold an ack, so the home has the acks of the last frame first.
const SPACING: Span = Span::from_nanos(30_000_000);

/// What the program and the peer tell the home.
#[derive(Default)]
struct Progress {
    /// The program's hello is admitted, so the home can accept the peer's session.
    admitted: AtomicBool,
    /// The reads that the home opened.
    opened: AtomicUsize,
    /// The whole frames that the program took.
    client: AtomicUsize,
    /// The whole frames that the peer took.
    wire: AtomicUsize,
}

/// A client read of `value` in `mode` and a HUB WIRE session of `time` and `value`
/// take each frame of one writer. Once warm, one poll of the client read, which sends
/// a frame, makes as many allocations as one poll of the session. Each frame is
/// taken, and its acks come, before the next is sent, so the transport's sends do not
/// depend on when the acks come.
pub(crate) fn relays_a_frame_with_the_allocations_of_a_wire_session(mode: Mode) {
    let progress = Arc::new(Progress::default());
    let mut sim = sim::Sim::new(sim::Config::default());
    let [home, program, other] =
        [(); 3].map(|()| sim.node(sim::node::Config::default()));
    let at = Address::Udp(SocketAddr::new(home.addresses()[0], net::PORT));
    let config = |name: &str| env::shards::Config {
        name: name.into(),
        core: None,
    };
    let (node, kept) = (program.clone(), Arc::clone(&progress));
    let main = move |tasks| read(node, tasks, at, mode, kept);
    drop(
        program
            .shards()
            .start(config("program"), main)
            .expect("starts"),
    );
    let (node, kept) = (other.clone(), Arc::clone(&progress));
    let main = move |tasks| session(node, tasks, at, mode, kept);
    drop(other.shards().start(config("peer"), main).expect("starts"));
    sim.run_on(&home, move |node, tasks| serve(node, tasks, mode, progress))
        .expect("the run ends");
}

/// The home: serves the program's read and the peer's session, writes each frame,
/// and counts one poll of each.
async fn serve(
    node: sim::node::Node,
    tasks: Tasks,
    mode: Mode,
    progress: Arc<Progress>,
) {
    let transport = net::transport(&node, &tasks, &net::own_pool(), net::HOME, 1 << 16);
    let (hub, now) = common::hub(&node, tasks.clone()).await;
    hub.set_rules(sessions::rules());
    let (program, hello) = net::accept(&transport).await;
    let link = hub.link(program.clone());
    let served = link.clone();
    tasks.spawn(async move { drop(served.serve(hello).await) });
    let mut client = pin!(link.serve(net::stream(&program).await));
    let (other, incoming) = net::accept(&transport).await;
    let remote = hub.link(other);
    let mut wire = pin!(remote.serve(incoming));
    let config = writer::Config {
        subject: common::name("a"),
        authority: Authority(1),
        lease: None,
        channels: vec![common::name("time"), common::name("value")],
    };
    let mut writer = hub.writer(config).await.expect("opens");
    while progress.opened.load(Ordering::Relaxed) < 2 {
        poll(client.as_mut());
        poll(wire.as_mut());
        node.clock().sleep(Span::MILLISECOND).await;
    }
    for n in 0..WARM + COUNTED {
        let draft = common::draft(&writer, now + n);
        common::write(&mut writer, draft);
        node.clock().sleep(SPACING).await;
        let counts = [client.as_mut(), wire.as_mut()]
            .map(|serving| ALLOCATOR.count(|| poll(serving)).1);
        taken(&node, &progress, n).await;
        if n >= WARM {
            assert_eq!(counts[0], counts[1], "frame {n} of a {mode:?} read");
        }
    }
}

/// Waits until the program and the peer have taken frame `n`.
///
/// # Panics
///
/// When they do not take it within [`TAKE`].
async fn taken(node: &sim::node::Node, progress: &Progress, n: i64) {
    let frames = usize::try_from(n + 1).expect("few frames");
    let end = node.clock().now() + TAKE;
    while progress.client.load(Ordering::Relaxed) < frames
        || progress.wire.load(Ordering::Relaxed) < frames
    {
        assert!(
            node.clock().now() < end,
            "the readers did not take frame {n}"
        );
        node.clock().sleep(Span::MILLISECOND).await;
    }
}

/// The program: admits a hello, then reads `value` in `mode`.
async fn read(
    node: sim::node::Node,
    tasks: Tasks,
    at: Address,
    mode: Mode,
    progress: Arc<Progress>,
) {
    let mut agent = Agent::dial(&node, tasks, at, common::name(SUBJECT)).await;
    agent.admit().await;
    progress.admitted.store(true, Ordering::Relaxed);
    let mut stream = agent.read(mode, &["value"]).await;
    let (_, _, reader) = stream.opened(mode).await;
    progress.opened.fetch_add(1, Ordering::Relaxed);
    take(&mut stream.receiver, reader, &progress.client).await;
}

/// The peer: once the program is admitted, opens a HUB WIRE session of `time` and
/// `value` in `mode`.
async fn session(
    node: sim::node::Node,
    tasks: Tasks,
    at: Address,
    mode: Mode,
    progress: Arc<Progress>,
) {
    while !progress.admitted.load(Ordering::Relaxed) {
        node.clock().sleep(Span::MILLISECOND).await;
    }
    let pool = net::own_pool();
    let transport = net::transport(&node, &tasks, &pool, net::PEER, 1 << 16);
    let session = transport
        .dial(net::public_key(&net::HOME), &[at])
        .await
        .expect("dials");
    let (class, mode) = match mode {
        Mode::Latest => (Class::Latest, wire::hub::Mode::Latest),
        Mode::Complete => (
            Class::Complete,
            wire::hub::Mode::Complete {
                limit_bytes: 1 << 40,
            },
        ),
    };
    let (mut sender, mut receiver) = session.open(class).await.expect("opens");
    peer::send(&pool, &mut sender, &wire::header::encode(Protocol::Hub)).await;
    let reader = peer::open(&pool, &mut sender, &mut receiver, mode, &[1, 2]).await;
    progress.opened.fetch_add(1, Ordering::Relaxed);
    take(&mut receiver, reader, &progress.wire).await;
}

/// Decodes each message on `receiver` with `reader`, and counts each whole frame in
/// `taken`, until it has taken each frame of the run.
async fn take(
    receiver: &mut Receiver,
    mut reader: wire::hub::Reader,
    taken: &AtomicUsize,
) {
    let frames = usize::try_from(WARM + COUNTED).expect("few frames");
    while taken.load(Ordering::Relaxed) < frames {
        let message = receiver.recv().await.expect("receives").expect("a message");
        let decoded = reader.decode(&message).expect("a valid message");
        let last = matches!(
            decoded,
            FromHome::Body { .. } | FromHome::Ends { last: true, .. }
        );
        if last && reader.body().is_none() {
            taken.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// One poll with a waker that does nothing.
///
/// # Panics
///
/// When the read ends.
fn poll(serving: Pin<&mut impl Future<Output = Result<Served, serve::Error>>>) {
    let mut cx = Context::from_waker(Waker::noop());
    if let Poll::Ready(served) = serving.poll(&mut cx) {
        panic!("the read ended: {served:?}");
    }
}
