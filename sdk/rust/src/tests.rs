use std::any::Any;
use std::net::Ipv6Addr;
use std::panic::AssertUnwindSafe;
use std::sync::mpsc;
use std::time::Duration;

use super::*;

/// Runs `body` on a local runtime, and gives its panic.
fn local<T>(body: impl Future<Output = T>) -> thread::Result<T> {
    let runtime = runtime();
    panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(body)))
}

fn runtime() -> tokio::runtime::LocalRuntime {
    Builder::new_current_thread()
        .enable_time()
        .build_local(LocalOptions::default())
        .unwrap()
}

fn message(payload: &(dyn Any + Send)) -> &str {
    payload.downcast_ref::<&str>().copied().unwrap_or_default()
}

#[test]
fn a_client_moves_to_and_is_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Client>();
}

#[test]
fn a_panic_in_a_task_ends_the_guarded_body() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        spawned
            .guard(async {
                tasks.spawn(async { panic!("the task failed") });
                tokio::time::sleep(Duration::from_secs(10)).await;
            })
            .await;
    });
    assert_eq!(message(&*outcome.unwrap_err()), "the task failed");
}

#[test]
fn a_task_that_ends_leaves_the_body_running() {
    let outcome = local(async {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        let (done, ended) = oneshot::channel();
        tasks.spawn(async {});
        spawned
            .guard(async {
                tasks.spawn(async { done.send(7).unwrap() });
                ended.await
            })
            .await
    });
    assert_eq!(outcome.unwrap(), Ok(7));
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
fn a_drop_ends_the_thread_and_waits_for_it() {
    let (ended, end) = mpsc::channel();
    let client = Client(Some(thread(move |receiver| {
        drop(receiver.blocking_recv());
        ended.send(()).unwrap();
    })));
    drop(client);
    assert_eq!(end.try_recv(), Ok(()));
}

#[test]
#[should_panic(expected = "the session failed")]
fn a_drop_resumes_a_panic_of_the_thread() {
    drop(Client(Some(thread(|_| panic!("the session failed")))));
}

#[test]
fn a_client_of_port_0_is_unroutable() {
    let address = SocketAddr::from((Ipv6Addr::LOCALHOST, 0));
    let peer = PrivateKey([7; 32]).public();
    let credentials = Credentials {
        node: types::node::Key::from_u128(1),
        public_key: peer,
        subject: "plant.operator".parse().unwrap(),
        private_key: PrivateKey([8; 32]),
    };
    let unreachable = transport::Error::Unreachable {
        peer,
        attempts: vec![(
            transport::Address::Udp(address),
            transport::Error::Unroutable,
        )],
    };
    assert_eq!(
        Client::connect(address, credentials).unwrap_err(),
        Error::Session(program::Error::Transport(unreachable))
    );
}
