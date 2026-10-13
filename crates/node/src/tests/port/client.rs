use std::net::{IpAddr, Ipv4Addr};

use wire::hub::client::{REFUSED, Refusal};

use crate::client::{self, reach};

use super::*;

/// What [`reach`] gave on a shard of `host` with a pool of `budget` bytes, once `sim`
/// has run for a minute. A node on `host` can run meanwhile.
fn reached(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
    budget: u64,
) -> Result<(), client::Error> {
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    shard(host, move |host, tasks| async move {
        let config = block::Config::new(budget).expect("the budget fits");
        let memory = block::Heap::new(config.reservation());
        let reached = reach(client::Config {
            files: host.files(),
            net: host.net(),
            clock: host.clock(),
            entropy: host.entropy(),
            tasks,
            pool: Rc::new(block::Pool::new(config, memory)),
        })
        .await;
        *slot.lock().unwrap() = Some(reached.map(drop));
    });
    assert_eq!(sim.run_for(Span::MINUTE), Ok(()));
    let reached = out.lock().unwrap().take();
    reached.expect("reach ends in a minute")
}

/// Starts a node on `host`, which [`keyed`] made, at `listen`, with `region`.
fn start(host: &sim::node::Node, listen: SocketAddr, region: Region) -> Node {
    Node::start(Config {
        listen,
        region,
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    })
}

/// Starts a node of the root region on a host that [`keyed`] made, runs it for a
/// second, and crashes it, which leaves `address` and `admin.key`.
fn crashed(sim: &mut sim::Sim) -> sim::node::Node {
    let host = keyed(sim, 2);
    let node = start(&host, listen(&host), Region::Stored);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    sim.crash(&host, sim::Crash::Process);
    drop(node);
    host
}

#[test]
fn reach_on_a_directory_where_no_node_ran_gives_not_running() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(
        reached(&mut sim, &host, 1 << 20),
        Err(client::Error::NotRunning)
    );
}

#[test]
fn reach_after_a_clean_stop_gives_not_running() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, listen(&host), Region::Stored);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    assert_eq!(
        reached(&mut sim, &host, 1 << 20),
        Err(client::Error::NotRunning)
    );
}

/// A node with no region writes no `admin.key`.
#[test]
fn reach_on_a_node_with_no_admin_key_gives_key() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, listen(&host), Region::Absent);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    assert_eq!(reached(&mut sim, &host, 1 << 20), Err(client::Error::Key));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
}

#[test]
fn reach_with_an_admin_key_that_no_node_wrote_gives_key() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = crashed(&mut sim);
    let written = sim.run_on(&host, |host, _| async move {
        let files = host.files();
        let mode = env::files::Mode::Write;
        let file = files.open(Path::new(crate::admin::FILE), mode).await?;
        let pool = pool();
        let mut block = pool.alloc(1).expect("a block");
        block.copy_from_slice(b"x");
        file.write_at(0, &[block.freeze()]).await
    });
    assert_eq!(written, Ok(Ok(())));
    assert_eq!(reached(&mut sim, &host, 1 << 20), Err(client::Error::Key));
}

#[test]
fn reach_whose_read_of_address_fails_gives_the_file_error() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = crashed(&mut sim);
    let read = env::files::Operation::ReadAt;
    host.fail_file(Path::new(crate::address::FILE), read);
    let reached = reached(&mut sim, &host, 1 << 20);
    let Err(client::Error::Files(env::files::Error::Io {
        path, operation, ..
    })) = reached
    else {
        panic!("a failed read gives Files: {reached:?}");
    };
    assert_eq!((path, operation), (PathBuf::from("address"), read));
}

#[test]
fn reach_with_a_pool_too_small_for_a_transport_gives_transport() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = crashed(&mut sim);
    let reached = reached(&mut sim, &host, 1024);
    assert!(
        matches!(
            reached,
            Err(client::Error::Transport(transport::Error::Config {
                field: "pool",
                ..
            }))
        ),
        "{reached:?}"
    );
}

#[test]
fn reach_after_a_crash_gives_unanswered_at_the_address() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = crashed(&mut sim);
    let reached = reached(&mut sim, &host, 1 << 20);
    let at = listen(&host);
    let error = ::hub::client::Error::Transport(transport::Error::Unreachable {
        peer: KEY.public(),
        attempts: vec![(Address::Udp(at), transport::Error::TimedOut)],
    });
    assert_eq!(reached, Err(client::Error::Unanswered { at, error }));
}

/// The node serves no hello of a program yet: it rejects the hub stream.
#[test]
fn reach_on_a_running_node_dials_its_bound_address() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let node = start(&host, listen(&host), Region::Stored);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    let reached = reached(&mut sim, &host, 1 << 20);
    let Err(client::Error::Unanswered { at, error }) = reached else {
        panic!("the node rejects the hub stream: {reached:?}");
    };
    assert_eq!(at, listen(&host));
    let code = Code(wire::header::REJECTED);
    assert_eq!(
        error,
        ::hub::client::Error::Transport(transport::Error::Reset { code })
    );
    node.stop();
    assert_eq!(sim.run(), Ok(()));
}

#[test]
fn reach_dials_the_loopback_ip_for_an_unspecified_ip() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT);
    let node = start(&host, any, Region::Stored);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    let reached = reached(&mut sim, &host, 1 << 20);
    let at = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), PORT);
    let error = ::hub::client::Error::Transport(transport::Error::Unreachable {
        peer: KEY.public(),
        attempts: vec![(Address::Udp(at), transport::Error::TimedOut)],
    });
    assert_eq!(reached, Err(client::Error::Unanswered { at, error }));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
}

/// A peer at the node's address that closes each session with a refusal.
#[test]
fn reach_whose_hello_the_node_refuses_gives_refused() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = crashed(&mut sim);
    let at = listen(&host);
    shard(&host, move |host, tasks| async move {
        let (transport, _pool) = transport_at(&host, tasks, KEY, at);
        let session = transport.accept().await.expect("a session");
        session.close(Code(REFUSED));
        host.clock().sleep(Span::MINUTE).await;
    });
    assert_eq!(
        reached(&mut sim, &host, 1 << 20),
        Err(client::Error::Refused(Refusal::Refused))
    );
}
