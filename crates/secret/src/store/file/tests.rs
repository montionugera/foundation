use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use env::files::{Files, Mode, Operation};
use types::time::Span;

use super::*;

/// Runs `body` on a shard of a new sim node, with the node's files.
fn on_sim<F>(body: impl FnOnce(sim::node::Node, Files) -> F + Send + 'static)
where
    F: Future<Output = ()> + 'static,
{
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, |node, _| {
        let files = node.files();
        body(node, files)
    })
    .expect("the run ends");
}

/// Makes the file `path` with `bytes`, in place of the one there.
async fn put(files: &Files, path: &str, bytes: &[u8]) {
    let path = Path::new(path);
    let dir = path.parent().expect("a file in a directory");
    files.create_dir(dir).await.expect("makes the directory");
    files.remove(path).await.expect("removes the old file");
    let len = u64::try_from(bytes.len()).expect("a short file");
    let file = files
        .open(path, Mode::Create { len })
        .await
        .expect("makes it");
    if !bytes.is_empty() {
        let pool = block::Config::new(1 << 18).expect("a budget that fits");
        let pool = block::Pool::heap(pool);
        let block = pool.copy(bytes).expect("the pool holds the bytes");
        file.write_at(0, &[block]).await.expect("writes");
    }
    file.close().await;
}

async fn get(store: &File, name: &str) -> Result<Vec<u8>, Error> {
    let name: Name = name.parse().expect("a name");
    let value = store.get(&name).await?;
    Ok(value.expose().to_vec())
}

fn store(files: Files) -> File {
    File::new(Reads::new(files), Path::new("secrets"))
}

fn missing(path: &str) -> Result<Vec<u8>, Error> {
    Err(Error::Missing {
        detail: format!("the file /data/{path} is not there. Write the value to it"),
    })
}

#[test]
fn reads_the_value_with_one_line_end_cut() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        let cases: [(&[u8], &[u8]); 8] = [
            (b"t", b"t"),
            (b"t\n", b"t"),
            (b"t\r\n", b"t"),
            (b"t\n\n", b"t\n"),
            (b"t\r\n\r\n", b"t\r\n"),
            (b"t\r", b"t\r"),
            (b"\nt", b"\nt"),
            (b" t ", b" t "),
        ];
        for (file, value) in cases {
            put(&files, "secrets/plant.token", file).await;
            assert_eq!(
                get(&store, "plant.token").await,
                Ok(value.to_vec()),
                "{file:?}"
            );
        }
    });
}

#[test]
fn gives_the_new_value_at_the_next_get_after_a_change() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        assert_eq!(
            get(&store, "plant.token").await,
            missing("secrets/plant.token")
        );
        put(&files, "secrets/plant.token", b"old").await;
        assert_eq!(get(&store, "plant.token").await, Ok(b"old".to_vec()));
        put(&files, "secrets/plant.token", b"newer").await;
        assert_eq!(get(&store, "plant.token").await, Ok(b"newer".to_vec()));
    });
}

#[test]
fn has_no_value_when_the_directory_or_the_file_is_not_there() {
    on_sim(|_, files| async move {
        let store = File::new(Reads::new(files.clone()), Path::new("keys/node"));
        assert_eq!(
            get(&store, "plant.token").await,
            missing("keys/node/plant.token")
        );
        put(&files, "keys/plant.token", b"t").await;
        assert_eq!(
            get(&store, "plant.token").await,
            missing("keys/node/plant.token")
        );
        put(&files, "keys/node/plant.other", b"t").await;
        assert_eq!(
            get(&store, "plant.token").await,
            missing("keys/node/plant.token")
        );
    });
}

#[test]
fn has_no_value_when_the_file_holds_only_a_line_end() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        let empty = Err(Error::Missing {
            detail:
                "the file /data/secrets/plant.token is empty. Write the value to it"
                    .into(),
        });
        for file in [&b""[..], b"\n", b"\r\n"] {
            put(&files, "secrets/plant.token", file).await;
            assert_eq!(get(&store, "plant.token").await, empty, "{file:?}");
        }
    });
}

#[test]
fn holds_no_name_with_a_capital_also_when_its_file_is_there() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        let capital = Err(Error::BadName {
            detail: "Rename the secret in lowercase letters, since the file store \
                     holds no name with a capital letter"
                .into(),
        });
        put(&files, "secrets/plant.Token", b"t").await;
        assert_eq!(get(&store, "plant.Token").await, capital);
        put(&files, "secrets/Plant.token", b"t").await;
        assert_eq!(get(&store, "Plant.token").await, capital);
    });
}

#[test]
fn reads_a_file_of_64_kib_and_refuses_one_byte_more() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        let full = vec![b'k'; 1 << 16];
        put(&files, "secrets/plant.key", &full).await;
        assert_eq!(get(&store, "plant.key").await, Ok(full.clone()));
        put(
            &files,
            "secrets/plant.key",
            &[full, b"\n".to_vec()].concat(),
        )
        .await;
        let large = Err(Error::Unavailable {
            detail:
                "the file /data/secrets/plant.key is 65537 bytes, more than 65536. \
                     Make it at most 65536 bytes"
                    .into(),
        });
        assert_eq!(get(&store, "plant.key").await, large);
    });
}

#[test]
fn reads_a_file_at_each_bound_of_a_block_size_class() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        let lens = [1, 64, 65, 256, 257, 4096, 4097, 49_152, 49_153, 65_535];
        for len in lens {
            let value = vec![b'k'; len];
            put(&files, "secrets/plant.key", &value).await;
            assert_eq!(get(&store, "plant.key").await, Ok(value), "{len} bytes");
        }
    });
}

#[test]
fn gives_a_failed_file_call_as_unavailable() {
    on_sim(|node, files| async move {
        let store = store(files.clone());
        put(&files, "secrets/plant.token", b"t").await;
        let path = Path::new("secrets/plant.token");
        for (operation, call) in
            [(Operation::Open, "open"), (Operation::ReadAt, "read_at")]
        {
            node.fail_file(path, operation);
            let failed = Err(Error::Unavailable {
                detail: format!(
                    "{call} of secrets/plant.token failed with OS error 5. Make \
                     /data/secrets/plant.token a file of at most 65536 bytes that the \
                     user of the node can read"
                ),
            });
            assert_eq!(get(&store, "plant.token").await, failed);
        }
        assert_eq!(get(&store, "plant.token").await, Ok(b"t".to_vec()));
    });
}

/// A waker that counts its wakes.
#[derive(Default)]
struct Count(AtomicU64);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn wakes(count: &Count) -> u64 {
    count.0.load(Ordering::Relaxed)
}

/// Polls `future` once with a waker of `count`.
fn poll_with<F: Future>(future: Pin<&mut F>, count: &Arc<Count>) -> Poll<F::Output> {
    let waker = Waker::from(Arc::clone(count));
    future.poll(&mut Context::from_waker(&waker))
}

/// Polls `future` once with the waker of the task.
async fn poll_once<F: Future>(future: &mut Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

/// Puts the files `plant.a`, `plant.b`, and `plant.c`, each with its last letter.
async fn put_three(files: &Files) {
    for name in ["a", "b", "c"] {
        put(files, &format!("secrets/plant.{name}"), name.as_bytes()).await;
    }
}

#[test]
fn a_second_get_waits_until_the_first_ends() {
    on_sim(|node, files| async move {
        let store = store(files.clone());
        put_three(&files).await;
        let mut first = pin!(get(&store, "plant.a"));
        let mut second = pin!(get(&store, "plant.b"));
        let count = Arc::new(Count::default());
        assert_eq!(poll_once(&mut first).await, Poll::Pending);
        for _ in 0..3 {
            assert_eq!(poll_with(second.as_mut(), &count), Poll::Pending);
            node.clock().sleep(Span::SECOND).await;
        }
        assert_eq!(wakes(&count), 0, "the second waits while the first reads");
        assert_eq!(first.await, Ok(b"a".to_vec()));
        assert_eq!(wakes(&count), 1, "the end of the first wakes the second");
        assert_eq!(second.await, Ok(b"b".to_vec()));
        let further = get(&store, "plant.a").await;
        assert_eq!(further, Ok(b"a".to_vec()), "the gets leave no turn");
    });
}

#[test]
fn a_dropped_get_that_reads_gives_the_turn_to_the_next() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        put_three(&files).await;
        let mut first = Box::pin(get(&store, "plant.a"));
        let mut second = pin!(get(&store, "plant.b"));
        let count = Arc::new(Count::default());
        assert_eq!(poll_once(&mut first.as_mut()).await, Poll::Pending);
        assert_eq!(poll_with(second.as_mut(), &count), Poll::Pending);
        drop(first);
        assert_eq!(wakes(&count), 1, "the drop of the first wakes the second");
        assert_eq!(second.await, Ok(b"b".to_vec()));
        let further = get(&store, "plant.a").await;
        assert_eq!(further, Ok(b"a".to_vec()), "the gets leave no turn");
    });
}

#[test]
fn a_dropped_get_that_waits_keeps_the_order_of_the_others() {
    on_sim(|_, files| async move {
        let store = store(files.clone());
        put_three(&files).await;
        let mut first = pin!(get(&store, "plant.a"));
        let mut second = Box::pin(get(&store, "plant.b"));
        let mut third = pin!(get(&store, "plant.c"));
        let (count_second, count_third) = (Arc::default(), Arc::default());
        assert_eq!(poll_once(&mut first).await, Poll::Pending);
        assert_eq!(poll_with(second.as_mut(), &count_second), Poll::Pending);
        assert_eq!(poll_with(third.as_mut(), &count_third), Poll::Pending);
        drop(second);
        assert_eq!(wakes(&count_third), 0, "the third waits for the first");
        assert_eq!(first.await, Ok(b"a".to_vec()));
        assert_eq!(wakes(&count_second), 0, "the dropped get left the order");
        assert_eq!(wakes(&count_third), 1, "the first wakes the third");
        assert_eq!(third.await, Ok(b"c".to_vec()));
        let further = get(&store, "plant.a").await;
        assert_eq!(further, Ok(b"a".to_vec()), "the gets leave no turn");
    });
}

/// The files of a new sim node, with the sim that drives them.
fn files() -> (sim::Sim, Files) {
    let mut sim = sim::Sim::new(sim::Config::default());
    let files = sim.node(sim::node::Config::default()).files();
    (sim, files)
}

#[test]
fn shows_its_directory_and_no_value_in_debug() {
    let (_sim, files) = files();
    let store = store(files);
    assert_eq!(format!("{store:?}"), "File { dir: \"secrets\", .. }");
}

#[test]
#[should_panic(expected = "a secret directory is relative with no `..`: /secrets")]
fn refuses_an_absolute_directory() {
    let (_sim, files) = files();
    drop(File::new(Reads::new(files), Path::new("/secrets")));
}

#[test]
#[should_panic(expected = "a secret directory is relative with no `..`: a/../secrets")]
fn refuses_a_directory_with_a_parent_segment() {
    let (_sim, files) = files();
    drop(File::new(Reads::new(files), Path::new("a/../secrets")));
}
