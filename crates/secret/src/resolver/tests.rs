use std::future::poll_fn;
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use env::files::{Absolute, Mode};
use types::name::Name;

use super::*;

/// The variables of each resolver of these tests.
fn vars() -> Arc<BTreeMap<OsString, OsString>> {
    Arc::new(BTreeMap::from([
        ("INFLUX_TOKEN".into(), "t0ken".into()),
        ("EMPTY".into(), OsString::new()),
    ]))
}

/// Runs `body` on a shard of a new sim node, with a resolver of [`vars`] and a
/// file store in `secrets`.
fn on_sim<F>(body: impl FnOnce(sim::node::Node, Resolver) -> F + Send + 'static)
where
    F: Future<Output = ()> + 'static,
{
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, |node, _| {
        let resolver = Resolver::new(vars(), node.files(), Path::new("secrets"));
        body(node, resolver)
    })
    .expect("the run ends");
}

async fn get(resolver: &Resolver, source: &Source) -> Result<Vec<u8>, Error> {
    Ok(resolver.get(source).await?.expose().to_vec())
}

fn env(name: &str) -> Source {
    Source::Env(name.into())
}

fn file(path: &str) -> Source {
    Source::File(Absolute::new(path).expect("an absolute path"))
}

fn missing(detail: &str) -> Result<Vec<u8>, Error> {
    Err(Error::Missing {
        detail: detail.into(),
    })
}

#[test]
fn reads_a_variable_and_says_how_to_set_one_that_is_not_set_or_empty() {
    on_sim(|_, resolver| async move {
        assert_eq!(
            get(&resolver, &env("INFLUX_TOKEN")).await,
            Ok(b"t0ken".to_vec())
        );
        assert_eq!(
            get(&resolver, &env("TOKEN")).await,
            missing(
                "the environment variable TOKEN of the node is not set. Start the node \
                 with it set to the value"
            )
        );
        assert_eq!(
            get(&resolver, &env("EMPTY")).await,
            missing(
                "the environment variable EMPTY of the node is empty. Start the node \
                 with it set to the value"
            )
        );
    });
}

#[test]
fn reads_a_variable_of_64_kib_and_refuses_one_byte_more() {
    let full = "k".repeat(1 << 16);
    let vars = BTreeMap::from([
        ("FULL".into(), OsString::from(&full)),
        ("OVER".into(), OsString::from(format!("{full}k"))),
    ]);
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let got = sim.run_on(&node, |node, _| async move {
        let resolver =
            Resolver::new(Arc::new(vars), node.files(), Path::new("secrets"));
        (
            get(&resolver, &env("FULL")).await,
            get(&resolver, &env("OVER")).await,
        )
    });
    let (full_got, over) = got.expect("the run ends");
    assert_eq!(full_got, Ok(full.into_bytes()));
    assert_eq!(
        over,
        Err(Error::Unavailable {
            detail: "the environment variable OVER of the node is 65537 bytes, more \
                     than 65536. Start the node with it set to at most 65536 bytes"
                .into(),
        })
    );
}

#[cfg(unix)]
#[test]
fn gives_the_bytes_of_a_variable_that_is_not_utf_8() {
    use std::os::unix::ffi::OsStringExt;

    let bytes = vec![b't', 0xff, 0xfe, b'k'];
    let vars = BTreeMap::from([("TOKEN".into(), OsString::from_vec(bytes.clone()))]);
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let got = sim.run_on(&node, |node, _| async move {
        let resolver =
            Resolver::new(Arc::new(vars), node.files(), Path::new("secrets"));
        get(&resolver, &env("TOKEN")).await
    });
    assert_eq!(got.expect("the run ends"), Ok(bytes));
}

#[test]
fn reads_an_absolute_file_with_one_line_end_cut() {
    on_sim(|node, resolver| async move {
        let paths = node.paths();
        let path = "/run/secrets/token";
        assert_eq!(
            get(&resolver, &file(path)).await,
            missing("the file /run/secrets/token is not there. Write the value to it")
        );
        for (bytes, value) in [(&b"t\n"[..], &b"t"[..]), (b"t\r\n\n", b"t\r\n")] {
            paths.write(Path::new(path), bytes).expect("writes");
            assert_eq!(get(&resolver, &file(path)).await, Ok(value.to_vec()));
        }
        for bytes in [&b""[..], b"\n", b"\r\n"] {
            paths.write(Path::new(path), bytes).expect("writes");
            assert_eq!(
                get(&resolver, &file(path)).await,
                missing("the file /run/secrets/token is empty. Write the value to it"),
                "{bytes:?}"
            );
        }
    });
}

#[test]
fn reads_an_absolute_file_of_64_kib_and_refuses_one_byte_more() {
    on_sim(|node, resolver| async move {
        let (paths, path) = (node.paths(), Path::new("/run/secrets/key"));
        let full = vec![b'k'; 1 << 16];
        paths.write(path, &full).expect("writes");
        assert_eq!(
            get(&resolver, &file("/run/secrets/key")).await,
            Ok(full.clone())
        );
        paths
            .write(path, &[full, b"\n".to_vec()].concat())
            .expect("writes");
        assert_eq!(
            get(&resolver, &file("/run/secrets/key")).await,
            Err(Error::Unavailable {
                detail:
                    "the file /run/secrets/key is 65537 bytes, more than 65536. Make \
                         it at most 65536 bytes"
                        .into(),
            })
        );
    });
}

#[test]
fn gives_a_failed_open_of_an_absolute_file_as_unavailable() {
    on_sim(|node, resolver| async move {
        let path = Path::new("/run/secrets/token");
        node.paths().write(path, b"t").expect("writes");
        node.fail_path(path, 13);
        assert_eq!(
            get(&resolver, &file("/run/secrets/token")).await,
            Err(Error::Unavailable {
                detail: "the open of the file /run/secrets/token failed with OS \
                         error 13. Make the file readable by the user of the node"
                    .into(),
            })
        );
    });
}

#[test]
fn reads_a_named_secret_from_the_store() {
    on_sim(|node, resolver| async move {
        let name: Name = "plant.token".parse().expect("a name");
        let source = Source::Secret(name);
        assert_eq!(
            get(&resolver, &source).await,
            missing(
                "the file /data/secrets/plant.token is not there. Write the value to it"
            )
        );
        put(&node.files(), b"t\n").await;
        assert_eq!(get(&resolver, &source).await, Ok(b"t".to_vec()));
    });
}

/// Makes `secrets/plant.token` in the data directory with `bytes`.
async fn put(files: &env::files::Files, bytes: &[u8]) {
    files
        .create_dir(Path::new("secrets"))
        .await
        .expect("makes it");
    let path = Path::new("secrets/plant.token");
    let len = u64::try_from(bytes.len()).expect("a short file");
    let file = files
        .open(path, Mode::Create { len })
        .await
        .expect("makes it");
    let pool = block::Pool::heap(block::Config::new(1 << 18).expect("fits"));
    let block = pool.copy(bytes).expect("the pool holds the bytes");
    file.write_at(0, &[block]).await.expect("writes");
    file.close().await;
}

/// A waker that counts its wakes.
#[derive(Default)]
struct Count(AtomicU64);

impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn a_file_read_waits_for_a_store_read_on_the_same_reads() {
    on_sim(|node, resolver| async move {
        put(&node.files(), b"s").await;
        node.paths()
            .write(Path::new("/run/secrets/token"), b"f")
            .expect("writes");
        let secret = Source::Secret("plant.token".parse().expect("a name"));
        let path = file("/run/secrets/token");
        let mut first = pin!(get(&resolver, &secret));
        let mut second = pin!(get(&resolver, &path));
        poll_fn(|cx| {
            assert!(first.as_mut().poll(cx).is_pending(), "the store reads");
            Poll::Ready(())
        })
        .await;
        let count = Arc::new(Count::default());
        let waker = Waker::from(Arc::clone(&count));
        let polled = second.as_mut().poll(&mut Context::from_waker(&waker));
        assert!(polled.is_pending(), "the file read waits for its turn");
        assert_eq!(first.await, Ok(b"s".to_vec()));
        assert_eq!(
            count.0.load(Ordering::Relaxed),
            1,
            "the store read wakes it"
        );
        assert_eq!(second.await, Ok(b"f".to_vec()));
    });
}

#[test]
fn shows_its_store_and_no_variable_in_debug() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let files = sim.node(sim::node::Config::default()).files();
    let resolver = Resolver::new(vars(), files, Path::new("secrets"));
    assert_eq!(
        format!("{resolver:?}"),
        "Resolver { store: File { dir: \"secrets\", .. }, .. }"
    );
}
