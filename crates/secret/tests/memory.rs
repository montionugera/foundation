//! The heap bytes that the gets of one resolver and its clones hold at once, through
//! its file store and absolute files, as SECRET STORES AS ADAPTERS states them. The
//! count covers the future of each get, as a task that boxes it holds it, and every
//! thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::collections::BTreeMap;
use std::future::poll_fn;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use env::files::{Absolute, Mode};
use secret::{Resolver, Source};

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// 5.4 MiB.
const BOUND: usize = 5_662_310;
/// The most heap bytes that a get holds while it waits for its turn.
const WAIT: usize = 1024;

fn main() {
    let cases = [
        (1, 1, 2, false),
        (4096, 1, 2, false),
        (1 << 16, 1, 2, false),
        (1 << 16, 2, 2, false),
        (1 << 16, 256, 2, false),
        (1 << 16, 1024, 2, false),
        (1 << 16, 1024, 1, false),
        (1 << 16, 1024, 2, true),
    ];
    let peaks = cases.map(|(len, gets, kinds, cloned)| {
        (len, gets, kinds, cloned, peak(len, gets, kinds, cloned))
    });
    let over: Vec<_> = peaks
        .iter()
        .filter(|&&(_, gets, _, _, peak)| peak > BOUND + (gets - 1) * WAIT)
        .collect();
    assert!(
        over.is_empty(),
        "gets held more than 5.4 MiB and 1 KiB for each that waits: {over:?}"
    );
}

/// The most heap bytes held at once during `gets` gets at once of a file of `len`
/// bytes, more than before them: from the store when `kinds` is 1, else half from
/// the store and half from an absolute file. Each get drops its value when it ends.
/// When `cloned`, each get goes through a clone of its own of the one resolver.
fn peak(len: usize, gets: usize, kinds: usize, cloned: bool) -> usize {
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
        let absolute = Path::new("/run/secrets/plant.key");
        node.paths()
            .write(absolute, &vec![b'k'; len])
            .expect("writes it");
        let resolver =
            Resolver::new(Arc::new(BTreeMap::new()), files, Path::new("secrets"));
        let sources = [
            Source::Secret("plant.key".parse().expect("a valid name")),
            Source::File(
                Absolute::new("/run/secrets/plant.key").expect("an absolute path"),
            ),
        ];
        let copies = if cloned { gets } else { 0 };
        let resolvers: Vec<Resolver> = (0..copies).map(|_| resolver.clone()).collect();
        let read = async |get: usize| {
            let resolver = resolvers.get(get).unwrap_or(&resolver);
            let value = resolver
                .get(&sources[get % kinds])
                .await
                .expect("the file holds the value");
            assert_eq!(value.expose().len(), len, "the get reads the whole file");
        };
        let before = ALLOCATOR.held();
        ALLOCATOR.reset_peak();
        let mut reads: Vec<Pin<Box<_>>> =
            (0..gets).map(|get| Box::pin(read(get))).collect();
        poll_fn(|cx| {
            reads.retain_mut(|read| read.as_mut().poll(cx).is_pending());
            if reads.is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        ALLOCATOR.peak() - before
    });
    run.expect("the run ends")
}
