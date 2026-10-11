use std::rc::Rc;

use super::*;
use crate::address::{self, Address, FILE, LEN};

/// The file that `sector::publish` renames to [`FILE`].
const NEW: &str = "address.new";

/// What a data directory that [`keyed`] made holds after a clean stop of a node with
/// no region on two cores.
const STOPPED: [&str; 7] = [
    "budget", "lock", "name", "node.key", "shard-0", "shard-1", "shards-2",
];

/// Starts a node with no region on `host`, which [`keyed`] made, with its port at
/// `port`.
fn start(host: &sim::node::Node, port: u16) -> Node {
    Node::start(Config {
        listen: SocketAddr::new(host.addresses()[0], port),
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    })
}

/// Stops `node`, runs `sim` until it ends, and gives what `join` gives.
fn stop(sim: &mut sim::Sim, node: Node) -> Result<(), Error> {
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    node.join()
}

/// Runs `body` on a shard of its own on `host` while `sim` runs for a millisecond,
/// and gives what it gave. A node on `host` can run meanwhile.
fn meanwhile<T: Send + 'static, F: Future<Output = T> + 'static>(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
    body: impl FnOnce(env::files::Files) -> F + Send + 'static,
) -> T {
    let out = Arc::new(Mutex::new(None));
    let (slot, own) = (Arc::clone(&out), host.clone());
    let shard = env::shards::Config {
        name: "reader".into(),
        core: None,
    };
    let started = host.shards().start(shard, move |_| async move {
        let value = body(own.files()).await;
        *slot.lock().unwrap() = Some(value);
    });
    drop(started.expect("the reader starts"));
    assert_eq!(sim.run_for(Span::MILLISECOND), Ok(()));
    let value = out.lock().unwrap().take();
    value.expect("the body ends in a millisecond")
}

/// The address in the file `address` of `host`, or `None` when the file is not
/// there. A node on `host` can run meanwhile.
///
/// # Panics
///
/// When the file is not one that a node wrote whole.
fn held(sim: &mut sim::Sim, host: &sim::node::Node) -> Option<Address> {
    meanwhile(sim, host, |files| async move {
        let file = match files.open(Path::new(FILE), env::files::Mode::Read).await {
            Err(env::files::Error::NotFound { .. }) => return None,
            file => file.expect("opens"),
        };
        assert_eq!(file.len(), LEN as u64, "a file of 99 bytes");
        let pool =
            block::Pool::heap(block::Config::new(4096).expect("the budget fits"));
        let into = pool.alloc(LEN).expect("a block");
        let bytes = file.read_at(0, into).await.expect("reads").to_vec();
        let bytes: [u8; LEN] = bytes.try_into().expect("a read of 99 bytes");
        Some(address::decode(&bytes).expect("a file that a node wrote"))
    })
}

/// The entries of the data directory of `host`, sorted. A node on `host` can run
/// meanwhile.
fn entries(sim: &mut sim::Sim, host: &sim::node::Node) -> Vec<PathBuf> {
    let mut entries = meanwhile(sim, host, |files| async move {
        files
            .list(Path::new(""))
            .await
            .expect("the directory lists")
    });
    entries.sort();
    entries
}

/// What [`entries`] gives while a node with no region runs on two cores.
fn running() -> Vec<PathBuf> {
    let mut running = vec![PathBuf::from(FILE)];
    running.extend(STOPPED.map(PathBuf::from));
    running
}

/// The address of a node on `host` that [`keyed`] made, with its port at `port`.
fn of(host: &sim::node::Node, port: u16) -> Address {
    Address {
        key: OWN,
        public_key: KEY.public(),
        bound: SocketAddr::new(host.addresses()[0], port),
    }
}

/// Starts a program on a new host of `sim` that dials `address`. Gives the result of
/// the dial once the run reaches it.
fn dial(
    sim: &mut sim::Sim,
    address: Address,
) -> Arc<Mutex<Option<Result<(), transport::Error>>>> {
    let peer = sim.node(sim::node::Config::default());
    let out = Arc::new(Mutex::new(None));
    let dialed = Arc::clone(&out);
    let shard = env::shards::Config {
        name: "peer".into(),
        core: None,
    };
    let own = peer.clone();
    let started = peer.shards().start(shard, move |tasks| async move {
        let config = block::Config::new(1 << 20).expect("the budget fits");
        let memory = block::Heap::new(config.reservation());
        let client = transport::Client::new(transport::client::Config {
            net: own.net(),
            clock: own.clock(),
            entropy: own.entropy(),
            tasks,
            pool: Rc::new(block::Pool::new(config, memory)),
        });
        let client = client.expect("a client");
        let at = [transport::Address::Udp(address.bound)];
        let session = client.dial(address.public_key, &at).await;
        *dialed.lock().unwrap() = Some(session.map(drop));
    });
    drop(started.expect("the peer starts"));
    out
}

/// A node that runs holds its key, the public key of its transport, and the address
/// that its port is bound at in `address`, and a program that dials them reaches it.
/// The file is gone after a clean stop.
#[test]
fn a_running_node_holds_its_keys_and_its_bound_address() {
    for (port, stored) in [(PORT, false), (0, false), (PORT, true), (0, true)] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let node = Node::start(Config {
            region: if stored {
                Region::Stored
            } else {
                Region::Absent
            },
            listen: SocketAddr::new(host.addresses()[0], port),
            ..config(&host, Size::MEBIBYTE, Box::new(heap))
        });
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        let found = held(&mut sim, &host).expect("a node that runs holds its address");
        if port == 0 {
            assert_ne!(found.bound.port(), 0, "stored {stored}");
        }
        let bound = if port == 0 { found.bound.port() } else { port };
        assert_eq!(found, of(&host, bound), "port {port}, stored {stored}");
        let dialed = dial(&mut sim, found);
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        let dialed = dialed.lock().unwrap().take();
        assert_eq!(dialed, Some(Ok(())), "port {port}, stored {stored}");
        assert_eq!(stop(&mut sim, node), Ok(()));
        assert_eq!(held(&mut sim, &host), None);
    }
}

/// A clean stop removes `address`, and leaves no `address.new`.
#[test]
fn a_clean_stop_removes_the_address() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, PORT);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    assert_eq!(entries(&mut sim, &host), running());
    assert_eq!(stop(&mut sim, node), Ok(()));
    assert_eq!(listed(&mut sim, &host, ""), STOPPED.map(PathBuf::from));
}

/// Whether a node holds `address` when it has run for `after`: what a process crash
/// then leaves.
fn published(after: Span) -> bool {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, PORT);
    assert_eq!(sim.run_for(after), Ok(()), "at {after:?}");
    sim.crash(&host, sim::Crash::Process);
    drop(node);
    held(&mut sim, &host).is_some()
}

/// A stop at any point of the start, then a probe that takes the lock as soon as it
/// is free: the node has removed `address` by then.
#[test]
fn the_lock_outlives_the_address() {
    let mut seen = [false; 2];
    for step in 0..600 {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let node = start(&host, PORT);
        let after = Span::from_nanos(step * 5_000);
        assert_eq!(sim.run_for(after), Ok(()), "at {after:?}");
        node.stop();
        let found = sim
            .run_on(&host, |host, _| async move {
                let (lock, _) = take_lock(&host).await;
                let files = host.files();
                let found = files.open(Path::new(FILE), env::files::Mode::Read).await;
                drop(lock);
                found.map(drop)
            })
            .expect("the probe ends");
        let absent = matches!(found, Err(env::files::Error::NotFound { .. }));
        assert!(absent, "at {after:?}: {found:?}");
        let busy = Error::Directory(env::files::Error::Busy {
            path: PathBuf::from("lock"),
        });
        // A probe that takes the lock before the claim refuses the node.
        let joined = node.join();
        assert!(joined == Ok(()) || joined == Err(busy), "at {after:?}");
        seen[usize::from(published(after))] = true;
    }
    assert_eq!(seen, [true; 2], "a stop before and after the publish");
}

/// A crash leaves `address`, and the next start writes its own in its place.
#[test]
fn a_crash_leaves_the_address_until_the_next_start_writes_its_own() {
    for crash in [sim::Crash::Process, sim::Crash::Power] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let node = start(&host, PORT);
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        sim.crash(&host, crash);
        drop(node);
        assert_eq!(held(&mut sim, &host), Some(of(&host, PORT)), "{crash:?}");
        let node = start(&host, PORT + 1);
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        let next = held(&mut sim, &host);
        assert_eq!(next, Some(of(&host, PORT + 1)), "{crash:?}");
        assert_eq!(stop(&mut sim, node), Ok(()));
        assert_eq!(listed(&mut sim, &host, ""), STOPPED.map(PathBuf::from));
    }
}

/// A start removes an `address.new` that a crash left.
#[test]
fn a_start_removes_a_new_address_that_a_crash_left() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    write(&mut sim, &host, NEW, vec![1; 7]);
    let node = start(&host, PORT);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    assert_eq!(held(&mut sim, &host), Some(of(&host, PORT)));
    assert_eq!(entries(&mut sim, &host), running());
    assert_eq!(stop(&mut sim, node), Ok(()));
    assert_eq!(listed(&mut sim, &host, ""), STOPPED.map(PathBuf::from));
}

/// A power cut at any time of a start leaves no `address` or the whole one.
#[test]
fn a_power_cut_during_a_start_leaves_no_address_or_the_whole_address() {
    let mut seen = [false; 2];
    for seed in 0..4 {
        for after in (0..4_000_000).step_by(25_000).map(Span::from_nanos) {
            let mut sim = sim::Sim::new(sim::Config {
                seed,
                ..sim::Config::default()
            });
            let host = keyed(&mut sim, 2);
            let node = start(&host, PORT);
            assert_eq!(sim.run_for(after), Ok(()), "seed {seed} at {after:?}");
            sim.crash(&host, sim::Crash::Power);
            drop(node);
            let held = held(&mut sim, &host);
            let whole = held == Some(of(&host, PORT));
            assert!(
                held.is_none() || whole,
                "seed {seed} at {after:?}: {held:?}"
            );
            seen[usize::from(whole)] = true;
        }
    }
    assert_eq!(seen, [true; 2], "a power cut before and after the publish");
}

/// A failed file call of the publish stops the node with its error, and the next
/// start publishes.
#[test]
fn a_failed_file_call_of_the_publish_stops_the_node() {
    use env::files::Operation::{Open, Remove, Rename, Sync, WriteAt};
    for (path, operation, text) in [
        (FILE, Remove, "remove of address failed with OS error 5"),
        (NEW, Remove, "remove of address.new failed with OS error 5"),
        (NEW, Open, "open of address.new failed with OS error 5"),
        (
            NEW,
            WriteAt,
            "write_at of address.new failed with OS error 5",
        ),
        (NEW, Sync, "sync of address.new failed with OS error 5"),
        (NEW, Rename, "rename of address.new failed with OS error 5"),
    ] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        host.fail_file(Path::new(path), operation);
        let node = start(&host, PORT);
        assert_eq!(sim.run(), Ok(()), "{operation:?} {path}");
        let error = Error::Directory(env::files::Error::Io {
            path: PathBuf::from(path),
            operation,
            code: 5,
        });
        assert_eq!(node.join(), Err(error.clone()), "{operation:?} {path}");
        assert_eq!(
            error.to_string(),
            format!("cannot use the data directory: {text}")
        );
        assert_eq!(held(&mut sim, &host), None, "{operation:?} {path}");
        let node = start(&host, PORT);
        assert_eq!(sim.run_for(Span::SECOND), Ok(()));
        let next = held(&mut sim, &host);
        assert_eq!(next, Some(of(&host, PORT)), "{operation:?} {path}");
        assert_eq!(stop(&mut sim, node), Ok(()), "{operation:?} {path}");
    }
}

/// A sync of the data directory that fails at any time of a start stops the node,
/// which holds no `address` once it has ended. The last sync of a start is the one
/// of the publish.
#[test]
fn a_failed_sync_of_the_data_directory_leaves_no_address() {
    let sync = env::files::Error::Io {
        path: PathBuf::new(),
        operation: env::files::Operation::SyncDir,
        code: 5,
    };
    let mut last = None;
    for step in 0..600 {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let node = start(&host, PORT);
        let after = Span::from_nanos(step * 5_000);
        assert_eq!(sim.run_for(after), Ok(()), "at {after:?}");
        host.fail_file(Path::new(""), env::files::Operation::SyncDir);
        assert_eq!(sim.run_for(Span::SECOND), Ok(()), "at {after:?}");
        let joined = stop(&mut sim, node);
        let files = ::buffer::Error::Files(sync.clone());
        let opened = |core| Error::Buffer {
            core,
            error: files.clone(),
        };
        let failed = [Error::Directory(sync.clone()), opened(0), opened(1)];
        match &joined {
            Ok(()) => {}
            Err(error) => {
                assert!(failed.contains(error), "at {after:?}: {error:?}");
                last = Some(error.clone());
            }
        }
        assert_eq!(held(&mut sim, &host), None, "at {after:?}");
        if step == 599 {
            assert_eq!(joined, Ok(()), "a start with no sync left");
        }
    }
    assert_eq!(
        last,
        Some(Error::Directory(sync)),
        "the sync of the publish"
    );
}

/// A remove of `address` that fails at the stop gives its error at `join` and leaves
/// the file, and the next start writes its own.
#[test]
fn a_failed_remove_at_the_stop_fails_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, PORT);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    host.fail_file(Path::new(FILE), env::files::Operation::Remove);
    let error = Error::Directory(env::files::Error::Io {
        path: PathBuf::from(FILE),
        operation: env::files::Operation::Remove,
        code: 5,
    });
    assert_eq!(stop(&mut sim, node), Err(error));
    assert_eq!(held(&mut sim, &host), Some(of(&host, PORT)));
    let node = start(&host, PORT + 1);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    assert_eq!(held(&mut sim, &host), Some(of(&host, PORT + 1)));
    assert_eq!(stop(&mut sim, node), Ok(()));
    assert_eq!(held(&mut sim, &host), None);
}
