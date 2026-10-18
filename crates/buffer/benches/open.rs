//! The time of `Buffer::open` of a full ring that trims, on one shard of a sim node.
//! Run with `cargo bench -p buffer --bench open`.
//!
//! Each case fills a ring once: each commit holds one data entry of each of its
//! indexes, until the ring wrapped twice. Then it times each open of the ring. Each
//! line gives the MiB of the ring, its indexes, and the median ms per open. The sim and
//! the fill are the same in each run, so two builds open the same records.
//!
//! The files are in memory and each call ends at once, so a figure is the CPU time of
//! the open: the walk and a copy of each byte of the ring in and out, as with a warm
//! page cache. It is not the start time of a node, which adds the reads of a disk. The
//! files of the sim node model a disk, which makes an open 4 to 11 times slower, so a
//! figure on them would measure the sim.
//!
//! A debug build, which `cargo test` makes, runs one small case, so a test run stays
//! fast.

#[path = "../tests/common/memory.rs"]
#[expect(dead_code, reason = "the bench uses only the driver")]
mod memory;

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use block::{Heap, Pool};
use buffer::{Buffer, Config, Entry, Layout, Parts, Samples};
use env::tasks::Tasks;
use memory::Memory;
use types::channel::{self, Slot, Slots};
use types::frame::Path;
use types::log::Mark;
use types::time::{Span, Stamp};

const WARMUP: usize = 1;
const ROUNDS: usize = 5;
const MIB: u64 = 1 << 20;
/// The commits of the fill between two checks of the wrap.
const CHECK: u64 = 256;

fn main() {
    let cases: &[(u64, u32)] = if cfg!(debug_assertions) {
        &[(1, 16)]
    } else {
        &[(256, 16), (256, 64), (256, 256), (1024, 64)]
    };
    let mut lines = Vec::new();
    for &(mib, indexes) in cases {
        let mut sim = sim::Sim::new(sim::Config {
            steps_max: u64::MAX,
            ..sim::Config::default()
        });
        let node = sim.node(sim::node::Config::default());
        let area = mib * MIB;
        let nanos = sim
            .run_on(&node, move |node, tasks| bench(node, tasks, area, indexes))
            .expect("the run ends");
        lines.push((mib, indexes, median(nanos)));
    }
    print(&lines);
}

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn print(lines: &[(u64, u32, u64)]) {
    println!("over {ROUNDS} opens");
    println!("{:>6} {:>8} {:>10}", "MiB", "indexes", "ms");
    for &(mib, indexes, nanos) in lines {
        let ms = format!("{}.{}", nanos / 1_000_000, nanos / 100_000 % 10);
        println!("{mib:>6} {indexes:>8} {ms:>10}");
    }
}

/// The ns of each timed open of the full ring of `area` bytes and `indexes` indexes.
async fn bench(
    node: sim::node::Node,
    tasks: Tasks,
    area: u64,
    indexes: u32,
) -> Vec<u64> {
    let files = Memory::default().files();
    let config = || Config {
        files: files.clone(),
        dir: PathBuf::from("shard-0"),
        pool: Rc::new(pool()),
        clock: node.clock(),
        tasks: tasks.clone(),
        entropy: node.entropy(),
        layout: Layout::new(area, 1 << 16).expect("the sizes make a ring"),
        commit: Span::from_nanos(10_000_000),
        trimmed: true,
    };
    fill(config(), indexes).await;
    let mut nanos = Vec::new();
    for round in 0..WARMUP + ROUNDS {
        let mut slots = Slots::new();
        let (buffer, took) = timed(Buffer::open(config(), &mut slots)).await;
        if round >= WARMUP {
            nanos.push(took);
        }
        end(buffer).await;
    }
    nanos
}

/// Commits one entry of each of `indexes` indexes until the ring wrapped twice: the
/// first index lost at least as many seqs as it keeps.
async fn fill(config: Config, indexes: u32) {
    let mut slots = Slots::new();
    let buffer = Buffer::open(config, &mut slots).await.expect("opens");
    let slots: Vec<(channel::Key, Slot)> = (0..indexes)
        .map(|index| {
            let key = channel::Key::from_u128(u128::from(index) + 1);
            (key, slots.index(key))
        })
        .collect();
    let reader = buffer.reader();
    let (_, first) = slots[0];
    let mut seq = 0;
    loop {
        for _ in 0..CHECK {
            commit(&buffer, &slots, seq).await;
            seq += 1;
        }
        let read = reader
            .read(first, Path::Live, Mark::at(0), 1)
            .await
            .expect("reads");
        if read.gap.is_some_and(|gap| 2 * gap.end >= seq) {
            break;
        }
    }
    end(buffer).await;
}

/// Commits one entry at `seq` of each index of `slots`.
async fn commit(buffer: &Buffer, slots: &[(channel::Key, Slot)], seq: u64) {
    let batch = slots.iter().map(|&(index, slot)| Entry {
        index,
        slot,
        path: Path::Live,
        first: seq,
        samples: Some(Samples {
            len: NonZeroU32::MIN,
            last: Some(Stamp::from_nanos(7)),
        }),
        stored_at: Stamp::from_nanos(7),
        tag: 0,
        parts: Parts::default(),
    });
    buffer.append(batch).expect("queues");
    buffer.committed().await.expect("commits");
}

/// The value of `future`, which must open, and the ns until it ends.
#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
async fn timed(
    future: impl Future<Output = Result<Buffer, buffer::Error>>,
) -> (Buffer, u64) {
    let started = Instant::now();
    let buffer = future.await.expect("opens");
    let took = u64::try_from(started.elapsed().as_nanos()).expect("fits");
    (buffer, took)
}

async fn end(buffer: Buffer) {
    let ending = buffer.ended();
    drop(buffer);
    ending.await.expect("ends");
}

fn pool() -> Pool {
    let config = block::Config::new(1 << 21).expect("the budget fits");
    Pool::new(config.clone(), Heap::new(config.reservation()))
}

fn median(mut nanos: Vec<u64>) -> u64 {
    nanos.sort_unstable();
    nanos[nanos.len() / 2]
}
