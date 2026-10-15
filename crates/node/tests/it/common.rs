//! Nodes and shards on the real OS, for the tests that start a node in the test
//! process.

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use env::files::Driver;
use transport::Address;
use types::byte::Size;

use crate::rig::Rig;

/// How long a test waits for a step of another thread.
pub(crate) const PATIENCE: Duration = Duration::from_secs(60);

/// Starts the node "edge" on the real OS in `rig`, which listens on `listen`. The
/// disk of each shard is `wrap` over the disk of `rig`.
pub(crate) fn start_node<D: Driver + Send + 'static>(
    rig: &Rig,
    shards: &env::shards::Shards,
    threads: &env::threads::Threads,
    io: &Rc<RefCell<Vec<env::thread::Handle>>>,
    listen: SocketAddr,
    region: node::Region,
    wrap: fn(os::Disk) -> D,
) -> node::Node {
    let cores = u64::try_from(shards.cores().get()).unwrap();
    let (dir, threads, io) = (rig.dir.clone(), threads.clone(), Rc::clone(io));
    node::Node::start(node::Config {
        shards: shards.clone(),
        clock: os::clock(),
        wall: os::wall().unwrap(),
        budget: node::Budget {
            pool: Size::from_bytes(cores * (512 << 10)),
            disk: Size::from_bytes(cores * (8 << 20)),
        },
        memory: Box::new(|len| Ok(block::Heap::new(len))),
        files: Box::new(move || {
            let disk = wrap(files(&dir, &threads, &io));
            Box::new(move || env::files::Files::new(disk))
        }),
        entropy: os::entropy(),
        net: os::net(),
        listen,
        region,
        name: "edge".parse().expect("a name"),
        settings: node::Settings::default(),
        vars: std::collections::BTreeMap::new(),
    })
}

/// What a shard sends on `receiver`.
///
/// # Panics
///
/// When it sends nothing within 60 s.
#[track_caller]
pub(crate) fn wait<T>(receiver: &mpsc::Receiver<T>) -> T {
    recv(receiver, PATIENCE).expect("the shard sends within 60 s")
}

/// What a shard sends on `receiver` within `timeout`.
///
/// # Errors
///
/// When it sends nothing within `timeout`, or the shard drops its sender.
pub(crate) fn recv<T>(
    receiver: &mpsc::Receiver<T>,
    timeout: Duration,
) -> Result<T, mpsc::RecvTimeoutError> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the test thread waits on a shard in real time"
    )]
    receiver.recv_timeout(timeout)
}

/// The disk of `dir` for one shard. `io` gets the handle of its I/O thread.
pub(crate) fn files(
    dir: &Path,
    threads: &env::threads::Threads,
    io: &RefCell<Vec<env::thread::Handle>>,
) -> os::Disk {
    let mut io = io.borrow_mut();
    let name = format!("io-{}", io.len());
    let (disk, thread) = os::files(dir, threads, &name).unwrap();
    io.push(thread);
    disk
}

/// A free UDP port of loopback.
pub(crate) fn free() -> SocketAddr {
    let at = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let port = transport::Port::bind(&os::net(), at).unwrap();
    let [Address::Udp(free)] = port.addresses()[..] else {
        panic!("invariant: a port has one UDP socket");
    };
    free
}

/// Starts shard `name` on `main`.
pub(crate) fn start<F: Future<Output = ()> + 'static>(
    shards: &env::shards::Shards,
    name: &str,
    main: impl FnOnce(env::tasks::Tasks) -> F + Send + 'static,
) -> env::thread::Handle {
    let config = env::shards::Config {
        name: name.into(),
        core: None,
    };
    shards.start(config, main).unwrap()
}
