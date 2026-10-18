//! Nodes and shards on the real OS, for the tests that start a node in the test
//! process.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::Range;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use env::files::Driver;
use env::net::Error;
use transport::Address;
use types::byte::Size;

use crate::rig::Rig;

/// How long a test waits for a step of another thread.
pub(crate) const PATIENCE: Duration = Duration::from_secs(60);

/// The loopback ports that no OS of the tests picks for a bind to port 0 or for a
/// connect: Linux picks from 32768, and macOS from 49152. So no such bind takes a port of
/// [`unpicked`] before the test binds it again.
pub(crate) const UNPICKED: Range<u16> = 1024..32768;

/// The config of the node "edge" on the real OS in `rig`, which listens on `listen`
/// and logs nothing. The disk of each shard is `wrap` over the disk of `rig`.
pub(crate) fn config<D: Driver + Send + 'static>(
    rig: &Rig,
    shards: &env::shards::Shards,
    threads: &env::threads::Threads,
    io: &Rc<RefCell<Vec<env::thread::Handle>>>,
    listen: SocketAddr,
    region: node::Region,
    wrap: impl Fn(os::Disk) -> D + Send + 'static,
) -> node::Config<block::Heap> {
    let cores = u64::try_from(shards.cores().get()).unwrap();
    let (dir, files_threads, io) = (rig.dir.clone(), threads.clone(), Rc::clone(io));
    node::Config {
        shards: shards.clone(),
        clock: os::clock(),
        wall: os::wall().unwrap(),
        budget: node::Budget {
            pool: Size::from_bytes(cores * (512 << 10)),
            disk: Size::from_bytes(cores * (8 << 20)),
        },
        memory: Box::new(|len| Ok(block::Heap::new(len))),
        files: Box::new(move || {
            let disk = wrap(files(&dir, &files_threads, &io));
            Box::new(move || env::files::Files::new(disk))
        }),
        entropy: os::entropy(),
        net: os::net(),
        listen,
        region,
        name: "edge".parse().expect("a name"),
        settings: node::Settings::default(),
        vars: std::collections::BTreeMap::new(),
        roots: os::roots::Roots::default(),
        log: node::Log {
            writer: Box::new(std::io::sink()),
            format: ops::Log::default(),
        },
        threads: threads.clone(),
    }
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

/// A free UDP port of loopback, of [`UNPICKED`].
pub(crate) fn free() -> SocketAddr {
    udp(&unpicked(|at| transport::Port::bind(&os::net(), at)))
}

/// The address of the UDP socket of `port`.
fn udp(port: &transport::Port) -> SocketAddr {
    let [Address::Udp(udp)] = port.addresses()[..] else {
        panic!("invariant: a port has one UDP socket");
    };
    udp
}

/// What `bind` gives at a free loopback port of [`UNPICKED`], from a random one, so that
/// two tests seldom try the same ports. Panics as [`unpicked_from`] does.
pub(crate) fn unpicked<T>(bind: impl FnMut(SocketAddr) -> Result<T, Error>) -> T {
    let ports = u64::from(UNPICKED.end - UNPICKED.start);
    let start = u16::try_from(os::entropy().rng().below(ports))
        .expect("a start below 31744 fits a u16");
    unpicked_from(UNPICKED.start + start, bind)
}

/// What `bind` gives at the first loopback port from `start` to the end of
/// [`UNPICKED`], then from its start, that is not in use. Panics on an error other than
/// [`Error::AddressInUse`], and when each port is in use.
fn unpicked_from<T>(
    start: u16,
    mut bind: impl FnMut(SocketAddr) -> Result<T, Error>,
) -> T {
    let mut ports = (start..UNPICKED.end).chain(UNPICKED.start..start);
    let bound = ports.find_map(|port| {
        let local = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        match bind(local) {
            Ok(bound) => Some(bound),
            Err(Error::AddressInUse { .. }) => None,
            Err(error) => panic!("bind at {local}: {error}"),
        }
    });
    bound.unwrap_or_else(|| panic!("each port of {UNPICKED:?} is in use"))
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

/// The first port of the range from which this host picks the port of a bind to port 0
/// or of a connect.
fn first_picked() -> u16 {
    #[cfg(target_os = "linux")]
    let range = std::fs::read_to_string("/proc/sys/net/ipv4/ip_local_port_range")
        .expect("Linux gives the range");
    #[cfg(target_os = "macos")]
    let range = {
        let output = std::process::Command::new("sysctl")
            .args(["-n", "net.inet.ip.portrange.first"])
            .output()
            .expect("macOS gives the range");
        String::from_utf8(output.stdout).expect("the range is text")
    };
    let first = range.split_whitespace().next().expect("a first port");
    first.parse().expect("the first port is a number")
}

#[test]
fn this_host_picks_no_port_of_unpicked() {
    let first = first_picked();
    assert!(
        UNPICKED.end <= first,
        "this host picks from port {first}, inside {UNPICKED:?}"
    );
}

#[test]
fn a_free_port_is_unpicked() {
    let port = free().port();
    assert!(
        UNPICKED.contains(&port),
        "port {port} is not in {UNPICKED:?}"
    );
}

#[test]
fn free_ports_start_at_a_random_port() {
    let ports: BTreeSet<u16> = (0..8).map(|_| free().port()).collect();
    assert!(ports.len() > 1, "eight picks gave one port: {ports:?}");
}

#[test]
#[should_panic(expected = "bind at 127.0.0.1:1024: network call failed with OS error")]
fn an_unpicked_bind_panics_on_an_error_other_than_in_use() {
    let unrouted = |at: SocketAddr| SocketAddr::from(([192, 0, 2, 1], at.port()));
    unpicked_from(UNPICKED.start, |at| {
        transport::Port::bind(&os::net(), unrouted(at))
    });
}

#[test]
fn an_unpicked_bind_from_a_taken_port_takes_a_free_one() {
    let bind = |at| transport::Port::bind(&os::net(), at);
    let taken = unpicked_from(UNPICKED.end - 1, bind);
    let free = unpicked_from(udp(&taken).port(), bind);
    let (taken, free) = (udp(&taken).port(), udp(&free).port());
    let other = free != taken && UNPICKED.contains(&free);
    assert!(other, "{free} after {taken}");
}
