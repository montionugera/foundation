//! The start sets the addresses of the card of the node.

use std::net::{IpAddr, Ipv4Addr, SocketAddrV6};

use ::mesh::card::addresses::Addresses;
use transport::Address;

use super::log::{Lines, to};
use super::port::mesh::{member, region};
use super::*;

/// The key and private key of a second node.
const OTHER: (types::node::Key, PrivateKey) =
    (types::node::Key::from_u128(2), PrivateKey([3; 32]));

/// What a start gives: the version and the addresses of the card of [`OWN`], read
/// `after` the start, and each line of the log of `node::card`.
#[derive(Debug, PartialEq)]
struct Started {
    version: u64,
    addresses: Vec<Address>,
    lines: Vec<String>,
}

/// Starts the node on `host` at `listen` in `region`, reads the card `after` the
/// start, then stops the node.
fn started_in(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
    listen: SocketAddr,
    region: Region,
    after: Span,
) -> Started {
    let lines = Lines::default();
    let node = Node::start(Config {
        region,
        listen,
        log: to(&lines, false, tracing::Level::WARN),
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    });
    let read = Arc::new(Mutex::new(None));
    let out = Arc::clone(&read);
    let clock = host.clock();
    node.operate(move |ops| async move {
        clock.sleep(after).await;
        let card = ops.mesh().member(OWN).expect("a member").card;
        let card = card.card();
        *out.lock().unwrap() = Some((card.version, card.addresses.as_slice().to_vec()));
    });
    assert_eq!(
        sim.run_for(Span::from_nanos(after.nanos() + Span::SECOND.nanos())),
        Ok(())
    );
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    let (version, addresses) = read.lock().unwrap().take().expect("the card read");
    let lines = lines
        .read()
        .into_iter()
        .filter(|line| line.contains(" node::card: "))
        .collect();
    Started {
        version,
        addresses,
        lines,
    }
}

/// [`started_in`] the region of the data directory, read a second after the start.
fn started(sim: &mut sim::Sim, host: &sim::node::Node, listen: SocketAddr) -> Started {
    started_in(sim, host, listen, Region::Stored, Span::SECOND)
}

/// The card holds the port of the last start: a start on another port puts it on the
/// card at the next version, and a start on the same port keeps the card.
#[test]
fn a_start_on_another_port_puts_that_port_on_the_card() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let ip = host.addresses()[0];
    for (port, version) in [(PORT, 1), (PORT + 1, 2), (PORT + 1, 2)] {
        let at = SocketAddr::new(ip, port);
        let found = started(&mut sim, &host, at);
        let expected = Started {
            version,
            addresses: vec![Address::Udp(at)],
            lines: Vec::new(),
        };
        assert_eq!(found, expected, "{port}");
    }
}

/// A first start on the unspecified IP founds a card with no address, and warns at
/// each start that no peer can dial the node.
#[test]
fn a_start_on_the_unspecified_ip_warns_that_no_peer_can_dial_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let any = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), PORT);
    let warned = |stamp: &str| {
        format!(
            "{stamp} WARN node::card: no peer can dial this node: give `--listen` an \
             address of this host bound=0.0.0.0:7000"
        )
    };
    assert_eq!(
        started(&mut sim, &host, any),
        Started {
            version: 1,
            addresses: Vec::new(),
            lines: vec![warned("2026-01-01T00:00:00.004956Z")],
        }
    );
    let found = started(&mut sim, &host, any);
    assert_eq!((found.version, found.addresses), (1, Vec::new()));
    assert_eq!(found.lines.len(), 1, "{:?}", found.lines);
}

/// A node bound to the link-local address of its host, with its scope, starts. Its
/// card holds no address, which means something only on the host, and it warns that
/// no peer can dial the node.
#[test]
fn a_start_on_a_scoped_address_keeps_it_off_the_card() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let (ip, interface) = host.link_local();
    let scoped = SocketAddr::V6(SocketAddrV6::new(ip, PORT, 0, interface));
    assert_eq!(
        started(&mut sim, &host, scoped),
        Started {
            version: 1,
            addresses: Vec::new(),
            lines: vec![
                "2026-01-01T00:00:00.004956Z WARN node::card: no peer can dial this \
                 node: give `--listen` an address of this host bound=[fe80::1%2]:7000"
                    .into()
            ],
        }
    );
}

/// Another change of the card less than a minute before the start: the start warns,
/// and sets the card a minute later.
#[test]
fn a_start_after_a_card_change_of_less_than_a_minute_sets_the_card_a_minute_later() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let ip = host.addresses()[0];
    assert_eq!(started(&mut sim, &host, listen(&host)).version, 1);
    let other = Address::Udp(SocketAddr::new(ip, PORT + 2));
    let set = Arc::new(Mutex::new(None));
    let lines = Lines::default();
    let at = SocketAddr::new(ip, PORT + 1);
    let node = Node::start(Config {
        region: Region::Stored,
        listen: at,
        log: to(&lines, false, tracing::Level::WARN),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    let out = Arc::clone(&set);
    let clock = host.clock();
    let read = Arc::new(Mutex::new(Vec::new()));
    let cards = Arc::clone(&read);
    node.operate(move |ops| async move {
        let addresses = Addresses::new(vec![other]).unwrap();
        *out.lock().unwrap() = Some(ops.mesh().set_addresses(addresses).await);
        for wait in [Span::from_nanos(50 * Span::SECOND.nanos()), Span::MINUTE] {
            clock.sleep(wait).await;
            let card = ops.mesh().member(OWN).expect("a member").card;
            let card = card.card();
            let held = (card.version, card.addresses.as_slice().to_vec());
            cards.lock().unwrap().push(held);
        }
    });
    let two = Span::from_nanos(2 * Span::MINUTE.nanos());
    assert_eq!(sim.run_for(two), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    assert_eq!(*set.lock().unwrap(), Some(Ok(())));
    assert_eq!(
        *read.lock().unwrap(),
        [(2, vec![other]), (4, vec![Address::Udp(at)])]
    );
    let warned: Vec<_> = lines
        .read()
        .into_iter()
        .filter(|line| line.contains(" node::card: "))
        .collect();
    assert_eq!(
        warned,
        [
            "2026-01-01T00:00:02.006291Z WARN node::card: the card of this node keeps \
          its addresses, and the node tries again in a minute error=\"a card of the \
          same member was proposed less than a minute ago; call again after a minute\""
        ]
    );
}

/// A stop during the minute of a card that waits ends the node at once, and logs
/// nothing more of the card.
#[test]
fn a_stop_during_the_minute_of_a_card_ends_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let ip = host.addresses()[0];
    assert_eq!(started(&mut sim, &host, listen(&host)).version, 1);
    let other = Address::Udp(SocketAddr::new(ip, PORT + 2));
    let lines = Lines::default();
    let node = Node::start(Config {
        region: Region::Stored,
        listen: SocketAddr::new(ip, PORT + 1),
        log: to(&lines, false, tracing::Level::WARN),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    node.operate(move |ops| async move {
        let addresses = Addresses::new(vec![other]).unwrap();
        assert_eq!(ops.mesh().set_addresses(addresses).await, Ok(()));
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    let clock = host.clock();
    let stopped = clock.now();
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    assert!(clock.now() < stopped + Span::SECOND, "{:?}", clock.now());
    let warned: Vec<_> = lines
        .read()
        .into_iter()
        .filter(|line| line.contains(" node::card: "))
        .collect();
    assert_eq!(
        warned,
        [
            "2026-01-01T00:00:02.006291Z WARN node::card: the card of this node keeps \
          its addresses, and the node tries again in a minute error=\"a card of the \
          same member was proposed less than a minute ago; call again after a minute\""
        ]
    );
}

/// A mesh whose log write fails during the call of a card a minute later fails the
/// node with that error, and logs nothing more of the card. The seed gives the order
/// of the end of the call and the stop of the group, both ready at one instant, so
/// the seeds cover both orders.
#[test]
fn a_mesh_that_stops_during_the_call_of_a_card_logs_no_more_of_it() {
    let soon = "WARN node::card: the card of this node keeps its addresses, and the \
                node tries again in a minute error=\"a card of the same member was \
                proposed less than a minute ago; call again after a minute\"";
    let mut waited = 0;
    for seed in 0..64 {
        let mut sim = sim::Sim::new(sim::Config {
            seed,
            ..sim::Config::default()
        });
        let host = keyed(&mut sim, 2);
        let ip = host.addresses()[0];
        assert_eq!(started(&mut sim, &host, listen(&host)).version, 1);
        let other = Address::Udp(SocketAddr::new(ip, PORT + 2));
        let lines = Lines::default();
        let node = Node::start(Config {
            region: Region::Stored,
            listen: SocketAddr::new(ip, PORT + 1),
            log: to(&lines, false, tracing::Level::WARN),
            ..config(&host, Size::MEBIBYTE, Box::new(heap))
        });
        let called = Arc::new(Mutex::new(None));
        let result = Arc::clone(&called);
        node.operate(move |ops| async move {
            let addresses = Addresses::new(vec![other]).unwrap();
            *result.lock().unwrap() = Some(ops.mesh().set_addresses(addresses).await);
        });
        assert_eq!(sim.run_for(Span::SECOND), Ok(()), "seed {seed}");
        // When the start's own call proposes first, no call of the card waits.
        if *called.lock().unwrap() == Some(Err(mesh::Error::Soon)) {
            node.stop();
            assert_eq!(sim.run(), Ok(()), "seed {seed}");
            assert_eq!(node.join(), Ok(()), "seed {seed}");
            continue;
        }
        assert_eq!(*called.lock().unwrap(), Some(Ok(())), "seed {seed}");
        let log = Path::new("mesh/log/log-0");
        host.fail_file(log, env::files::Operation::WriteAt);
        assert_eq!(sim.run(), Ok(()), "seed {seed}");
        let log = mesh::log::Error::Files(env::files::Error::Io {
            path: log.to_path_buf(),
            operation: env::files::Operation::WriteAt,
            code: 5,
        });
        let stopped = Err(Error::Group(mesh::Stopped::Write(log)));
        assert_eq!(node.join(), stopped, "seed {seed}");
        let warned: Vec<_> = lines
            .read()
            .into_iter()
            .filter(|line| line.contains(" node::card: "))
            .map(|line| line.split_once(' ').unwrap().1.to_owned())
            .collect();
        assert_eq!(warned, [soon], "seed {seed}");
        waited += 1;
    }
    assert_eq!(waited, 28);
}

/// A member that is not a voter keeps the addresses of its card, and warns of it.
#[test]
fn a_member_that_is_not_a_voter_warns_that_its_card_keeps_its_addresses() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let hosts = [keyed(&mut sim, 2), keyed(&mut sim, 2)];
    let mut founding = region(&[
        member(OWN, &KEY, &hosts[0]),
        member(OTHER.0, &OTHER.1, &hosts[1]),
    ]);
    founding.voters.remove(&OWN);
    let at = SocketAddr::new(hosts[0].addresses()[0], PORT + 1);
    let found = started_in(
        &mut sim,
        &hosts[0],
        at,
        Region::Given(founding),
        Span::SECOND,
    );
    assert_eq!(
        found,
        Started {
            version: 1,
            addresses: vec![Address::Udp(listen(&hosts[0]))],
            lines: vec![
                "2026-01-01T00:00:00.004201Z WARN node::card: the card of this node \
                 keeps its addresses error=\"this node is not a voter, and only a \
                 voter proposes a change\""
                    .into()
            ],
        }
    );
}

/// The one voter of its mesh counts its cards in memory, so each start that changes its
/// port proposes its card at once, also within a minute of the last one. A start on the
/// port of the last start proposes none.
#[test]
fn each_start_that_changes_the_port_of_the_one_voter_proposes_its_card() {
    for (ports, versions) in [
        (vec![PORT, PORT + 1, PORT], vec![1, 2, 3]),
        (vec![PORT, PORT + 1, PORT + 1, PORT], vec![1, 2, 2, 3]),
    ] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let ip = host.addresses()[0];
        let clock = host.clock();
        let begun = clock.now();
        let started: Vec<_> = ports
            .into_iter()
            .map(|port| started(&mut sim, &host, SocketAddr::new(ip, port)))
            .collect();
        assert!(clock.now() < begun + Span::MINUTE, "{:?}", clock.now());
        assert_eq!(
            started
                .iter()
                .map(|started| started.version)
                .collect::<Vec<_>>(),
            versions
        );
        assert!(started.iter().all(|started| started.lines.is_empty()));
    }
}
