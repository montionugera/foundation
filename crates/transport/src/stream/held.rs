//! The sent blocks that a session holds when acknowledgments come late or out of
//! order.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use block::{Heap, Pool};
use types::time::Span;

use crate::Class;
use crate::stream::Part;
use crate::testing::{self, spans};

/// The bytes of each block that the server sends from.
const BLOCK: usize = 64 << 10;
/// The shortest run that the carrier holds as a view of its block.
const VIEW_MIN: usize = 1453;

/// A pool of 32 MiB for the blocks that the server sends.
fn send_pool() -> Rc<Pool> {
    let config = block::Config::new(1 << 25).expect("the budget fits");
    let memory = Heap::new(config.reservation());
    Rc::new(Pool::new(config, memory))
}

/// A link of 10 ms with no rate limit, on which each packet waits up to `jitter`
/// more, so packets sent together come out of order.
fn link(jitter: Span) -> sim::link::Config {
    sim::link::Config {
        delay: spans(Span::MILLISECOND, 10),
        jitter,
        ..sim::link::Config::default()
    }
}

/// The most blocks that the send pool held at one time while the server sent
/// `count` messages on one stream of a session with `window` bytes, each the first
/// `part` bytes of a new block of [`BLOCK`] bytes, over links with `jitter`. Panics
/// when the client did not get each message.
fn held(seed: u64, window: usize, part: usize, count: usize, jitter: Span) -> usize {
    let got = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let (counted, read, out) = (Arc::clone(&got), Arc::clone(&got), Arc::clone(&peak));
    let (mut sim, client, server) = testing::sessions(
        seed,
        move |mut config| {
            config.window_bytes = window;
            config
        },
        move |side| async move {
            let mut incoming = side.session.accept().await.expect("a stream");
            while counted.load(Ordering::Relaxed) < count {
                let message = incoming.receiver.recv().await.expect("reads");
                assert_eq!(message.map(|message| message.len()), Some(part));
                counted.fetch_add(1, Ordering::Relaxed);
            }
        },
        move |side| async move {
            let pool = send_pool();
            let clock = side.node.clock();
            let most = Cell::new(0);
            let done = Cell::new(false);
            let send = async {
                let mut sender = side
                    .session
                    .open_sender(Class::Complete)
                    .await
                    .expect("a stream");
                for _ in 0..count {
                    let block = testing::alloc(&pool, BLOCK).expect("room").freeze();
                    let parts = [Part {
                        range: 0..part,
                        zeros: 0,
                    }];
                    sender.send_parts(block, &parts).await.expect("sends");
                    most.set(most.get().max(pool.committed()));
                }
                while read.load(Ordering::Relaxed) < count {
                    clock.sleep(spans(Span::MICROSECOND, 100)).await;
                }
                done.set(true);
            };
            let watch = async {
                while !done.get() {
                    most.set(most.get().max(pool.committed()));
                    clock.sleep(spans(Span::MICROSECOND, 100)).await;
                }
            };
            testing::join(send, watch).await;
            out.store(most.get(), Ordering::Relaxed);
        },
    );
    sim.link(&server, &client, link(jitter));
    sim.link(&client, &server, link(jitter));
    sim.run().expect("the run ends");
    assert_eq!(got.load(Ordering::Relaxed), count);
    peak.load(Ordering::Relaxed) / BLOCK
}

#[test]
fn a_gap_in_the_acknowledgments_holds_at_most_the_window() {
    let window = 1 << 20;
    for seed in 0..4 {
        let held = held(seed, window, BLOCK, 16 * window / BLOCK, Span::MICROSECOND);
        assert!(held <= window / BLOCK + 2, "seed {seed}: {held} blocks");
    }
}

#[test]
fn a_part_holds_its_block_while_a_byte_of_it_is_in_the_window() {
    let window = 256 << 10;
    let most = window / VIEW_MIN + 2;
    let held = held(0, window, VIEW_MIN, 4 * most, Span::ZERO);
    assert!(held <= most, "{held} blocks");
}
