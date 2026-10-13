use env::files::Operation;
use proptest::prelude::*;

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
        let pool = block::Pool::heap(pool(bytes.len()));
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
    File::new(files, Path::new("secrets"))
}

fn missing(path: &str) -> Result<Vec<u8>, Error> {
    Err(Error::Missing {
        detail: format!("write it to the file /data/{path}"),
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
        let store = File::new(files.clone(), Path::new("keys/node"));
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
        let capital = Err(Error::Missing {
            detail: "the file store holds no name with a capital letter. Rename the \
                     secret in lowercase letters"
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
            detail: "the file /data/secrets/plant.key is 65537 bytes, more than 65536"
                .into(),
        });
        assert_eq!(get(&store, "plant.key").await, large);
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
                detail: format!("{call} of secrets/plant.token failed with OS error 5"),
            });
            assert_eq!(get(&store, "plant.token").await, failed);
        }
        assert_eq!(get(&store, "plant.token").await, Ok(b"t".to_vec()));
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
    drop(File::new(files, Path::new("/secrets")));
}

#[test]
#[should_panic(expected = "a secret directory is relative with no `..`: a/../secrets")]
fn refuses_a_directory_with_a_parent_segment() {
    let (_sim, files) = files();
    drop(File::new(files, Path::new("a/../secrets")));
}

proptest! {
    #[test]
    fn a_pool_holds_a_file_of_each_length(len in 0..=1_usize << 16) {
        let pool = block::Pool::heap(pool(len));
        prop_assert_eq!(pool.alloc(len).map(|block| block.len()), Ok(len));
    }
}
