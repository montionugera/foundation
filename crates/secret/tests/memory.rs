//! The heap bytes that a get of the file store holds, as SECRET STORES AS ADAPTERS
//! states them. The count covers every thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::path::Path;

use env::files::Mode;
use secret::store::{File, Store};
use types::name::Name;

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// 5.4 MiB.
const BOUND: usize = 5_662_310;

fn main() {
    let peaks = [1, 4096, 1 << 16].map(|len| (len, peak(len)));
    let over: Vec<_> = peaks.iter().filter(|(_, peak)| *peak > BOUND).collect();
    assert!(over.is_empty(), "gets held more than 5.4 MiB: {over:?}");
}

/// The most heap bytes held at once during a get of a file of `len` bytes, more than
/// before it.
fn peak(len: usize) -> usize {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = sim.run_on(&node, move |node, _| async move {
        let files = node.files();
        let path = Path::new("secrets/plant.key");
        files
            .create_dir(Path::new("secrets"))
            .await
            .expect("makes it");
        let len_u64 = u64::try_from(len).expect("fits");
        let file = files.open(path, Mode::Create { len: len_u64 }).await;
        let file = file.expect("makes it");
        let pool = block::Pool::heap(block::Config::new(1 << 18).expect("fits"));
        let block = pool.copy(&vec![b'k'; len]).expect("holds it");
        file.write_at(0, &[block]).await.expect("writes it");
        file.close().await;
        drop(pool);
        let store = File::new(files, Path::new("secrets"));
        let name: Name = "plant.key".parse().expect("a valid name");
        let before = ALLOCATOR.held();
        ALLOCATOR.reset_peak();
        let value = store.get(&name).await.expect("the file holds the value");
        let peak = ALLOCATOR.peak() - before;
        assert_eq!(value.expose().len(), len, "the get reads the whole file");
        peak
    });
    run.expect("the run ends")
}
