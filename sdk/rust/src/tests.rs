use std::net::Ipv6Addr;
use std::time::Duration;

use super::*;

#[test]
fn a_client_moves_to_and_is_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Client>();
}

#[test]
fn the_pool_holds_a_datagram() {
    let pool = block::Pool::heap(POOL);
    assert!(pool.largest() >= 1472, "{}", pool.largest());
}

#[test]
fn the_frames_pool_holds_a_frame_of_32_kib() {
    let pool = block::Pool::heap(FRAMES);
    assert!(pool.largest() >= 32 << 10, "{}", pool.largest());
}

#[test]
fn each_error_names_its_cause() {
    let thread = Error::Thread {
        reason: "out of memory".into(),
    };
    let bind = Error::Bind(transport::Error::TimedOut);
    let session = Error::Session(program::Error::Unanswered);
    assert_eq!(
        [thread, bind, session].map(|e| e.to_string()),
        [
            "cannot start the client thread: out of memory",
            "cannot bind the local UDP socket: the peer stopped answering",
            "the node finished a stream with no answer",
        ]
    );
}

/// A thread of a client that runs `body` with the receiver of its end.
fn thread(body: impl FnOnce(oneshot::Receiver<()>) + Send + 'static) -> Thread {
    let (end, ended) = oneshot::channel();
    #[expect(clippy::disallowed_methods, reason = "a thread of a client")]
    let handle = thread::spawn(move || body(ended));
    Thread { end, handle }
}

#[test]
fn a_close_ends_the_thread_and_waits_for_it() {
    let (ended, end) = mpsc::channel();
    let client = Client(Some(thread(move |receiver| {
        drop(receiver.blocking_recv());
        ended.send(()).unwrap();
    })));
    client.close();
    assert_eq!(end.try_recv(), Ok(()));
}

#[test]
#[should_panic(expected = "the session failed")]
fn a_close_resumes_a_panic_of_the_thread() {
    Client(Some(thread(|_| panic!("the session failed")))).close();
}

#[test]
fn a_drop_ends_the_thread_and_does_not_wait() {
    let (go, wait) = mpsc::channel::<()>();
    let (ended, end) = mpsc::channel();
    let client = Client(Some(thread(move |receiver| {
        drop(receiver.blocking_recv());
        #[expect(clippy::disallowed_methods, reason = "the thread waits for the test")]
        wait.recv().unwrap();
        ended.send(()).unwrap();
    })));
    drop(client);
    go.send(()).unwrap();
    #[expect(clippy::disallowed_methods, reason = "the test waits for the thread")]
    let outcome = end.recv_timeout(Duration::from_secs(10));
    assert_eq!(outcome, Ok(()));
}

/// Connects to a node at `addresses`, and checks that the error names each one with
/// `cause`.
fn unreachable(addresses: Vec<SocketAddr>, cause: &transport::Error) {
    let peer = PrivateKey([7; 32]).public();
    let node = Node {
        addresses: addresses.clone(),
        key: types::node::Key::from_u128(1),
        public_key: peer,
    };
    let subject = Subject {
        name: "plant.operator".parse().unwrap(),
        private_key: PrivateKey([8; 32]),
    };
    let attempts = addresses
        .into_iter()
        .map(|address| (transport::Address::Udp(address), cause.clone()))
        .collect();
    let Err(error) = Client::connect(&node, subject) else {
        panic!("a client of an unreachable node connected");
    };
    let unreachable = transport::Error::Unreachable { peer, attempts };
    assert_eq!(
        error,
        Error::Session(program::Error::Transport(unreachable))
    );
}

/// Port 0 of IPv6 loopback.
fn port_0() -> SocketAddr {
    SocketAddr::from((Ipv6Addr::LOCALHOST, 0))
}

#[test]
fn a_client_of_port_0_is_unroutable() {
    unreachable(vec![port_0()], &transport::Error::Unroutable);
}

#[test]
fn a_client_of_a_node_with_no_address_is_unreachable_with_no_attempt() {
    unreachable(vec![], &transport::Error::Unroutable);
}

#[test]
fn a_connect_inside_a_tokio_runtime_blocks_and_gives_its_error() {
    let runtime = Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        unreachable(vec![port_0()], &transport::Error::Unroutable);
    });
}
