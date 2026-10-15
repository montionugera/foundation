//! A read keeps its list of chunks and the buffer of its message across polls, so no
//! allocation repeats on each poll. A message that comes over at least 64 polls makes
//! at most one allocation more than a read of the whole message in one poll. A
//! message whose prefix a packet splits after a full list makes at most one. A whole
//! message that waits for a block makes no allocation after its first poll, and the
//! read of a short message after it makes none: the reader keeps its list.

use std::net::SocketAddr;
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex};

use env::clock::Clock;
use sim::Sim;
use sim::node::Node;
use transport::Transport;
use types::time::Span;

use crate::common::{PORT, SERVER, config, fill, filled, part};
use crate::{READ, SHORT, once, send};

/// The time between two polls of a read while its message comes.
const EVERY: Span = Span::from_nanos(20_000);
/// The polls of a whole message that waits for a block.
const WAITS: usize = 8;
/// A message after which, in this sim, a packet splits the prefix of the next one
/// once the read's list is full.
const SPLIT: usize = 81_468;
/// The message after [`SPLIT`].
const NEXT: usize = 100_000;

/// Whether each poll of the server's reads was ready, and the allocations it made.
type Out = Vec<(bool, u64)>;

/// How the server reads the long message.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Once it is in, in one poll.
    Whole,
    /// Each [`EVERY`] from when the stream comes.
    Coming,
    /// Once it is in, [`WAITS`] polls with no block in the pool, then one with blocks.
    Waiting,
    /// As [`Mode::Coming`], then the next message the same way. Only the polls of the
    /// next message count.
    Next,
}

pub(crate) fn main() {
    for len in [100_000, 240_000] {
        let whole = run([len, SHORT], Mode::Whole);
        let [(true, whole)] = whole[..] else {
            panic!("{len} bytes: the whole message in one poll gave {whole:?}");
        };
        let coming = run([len, SHORT], Mode::Coming);
        let polls = coming.len();
        let allocated: u64 = coming.iter().map(|&(_, allocated)| allocated).sum();
        assert!(polls >= 64, "{len} bytes: {polls} polls");
        assert!(
            allocated <= whole + 1,
            "{len} bytes: {allocated} allocations over {polls} polls, against {whole} \
             in one poll"
        );
        let waiting = run([len, SHORT], Mode::Waiting);
        let [(false, _), ref waits @ .., (true, block), (true, short)] = waiting[..]
        else {
            panic!("{len} bytes: the waiting reads gave {waiting:?}");
        };
        assert_eq!(
            waits,
            [(false, 0); WAITS - 1],
            "{len} bytes: the polls after the first of a message that waits for a block"
        );
        assert_eq!(
            (block, short),
            (0, 0),
            "{len} bytes: the read with a block, and of the short message after it"
        );
    }
    let next = run([SPLIT, NEXT], Mode::Next);
    let allocated: u64 = next.iter().map(|&(_, allocated)| allocated).sum();
    assert!(
        allocated <= 1,
        "{allocated} allocations over {} polls of a message whose prefix a packet \
         splits after a full list",
        next.len()
    );
}

/// The [`Out`] of the server's reads in `mode` of the messages of `lens` bytes on one
/// stream.
fn run(lens: [usize; 2], mode: Mode) -> Out {
    let mut sim = Sim::new(sim::Config::default());
    let client = sim.node(sim::node::Config::default());
    let server = sim.node(sim::node::Config::default());
    let address = SocketAddr::new(server.addresses()[0], PORT);
    let out = Arc::new(Mutex::new(Vec::new()));
    serve(&server, lens[0], mode, Arc::clone(&out));
    send(&mut sim, &client, address, move |pool| {
        lens.map(|len| filled(pool, len)).into()
    });
    out.lock().expect("not poisoned").clone()
}

/// Starts the server on `node`. It reads the first message, of `len` bytes, in `mode`,
/// and puts the [`Out`] of its polls in `out`.
fn serve(node: &Node, len: usize, mode: Mode, out: Arc<Mutex<Out>>) {
    let own = node.clone();
    let shard = env::shards::Config {
        name: "server".into(),
        core: None,
    };
    let started = node.shards().start(shard, move |tasks| async move {
        let config = config(&own, tasks, SERVER);
        let full = matches!(mode, Mode::Waiting).then(|| fill(&config.pool, len));
        let transport = Transport::new(config, part(&own, PORT)).expect("a transport");
        let session = transport.accept().await.expect("a session");
        let mut receiver = session.accept().await.expect("a stream").receiver;
        let clock = own.clock();
        let mut polls = Vec::new();
        {
            let mut recv = pin!(receiver.recv());
            match mode {
                Mode::Whole => {
                    clock.sleep(READ).await;
                    polls.push(ready(recv.as_mut()));
                }
                Mode::Coming => polls = coming(recv.as_mut(), &clock).await,
                Mode::Next => drop(coming(recv.as_mut(), &clock).await),
                Mode::Waiting => {
                    clock.sleep(READ).await;
                    for _ in 0..WAITS {
                        polls.push(ready(recv.as_mut()));
                    }
                    drop(full);
                    polls.push(ready(recv.as_mut()));
                }
            }
        }
        match mode {
            Mode::Waiting => polls.push(ready(pin!(receiver.recv()))),
            Mode::Next => polls = coming(pin!(receiver.recv()), &clock).await,
            Mode::Whole | Mode::Coming => {}
        }
        *out.lock().expect("not poisoned") = polls;
    });
    drop(started.expect("a shard"));
}

/// Polls `read` each [`EVERY`] until it is ready, and gives the [`Out`] of its polls.
async fn coming<T>(mut read: Pin<&mut impl Future<Output = T>>, clock: &Clock) -> Out {
    let mut polls = Vec::new();
    loop {
        let poll = ready(read.as_mut());
        polls.push(poll);
        if poll.0 {
            return polls;
        }
        clock.sleep(EVERY).await;
    }
}

/// Whether one poll of `read` is ready, and the allocations it makes.
fn ready<T>(read: Pin<&mut impl Future<Output = T>>) -> (bool, u64) {
    let (poll, allocated) = once(read);
    (poll.is_ready(), allocated)
}
