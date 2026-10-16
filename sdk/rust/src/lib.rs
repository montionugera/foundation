//! The Rust SDK: a program's session with one Foundation node.

mod host;

use std::cell::{Cell, RefCell};
use std::fmt;
use std::future::poll_fn;
use std::net::SocketAddr;
use std::panic;
use std::pin::pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, JoinHandle};

use env::tasks::{Task, Tasks};
use tokio::runtime::{Builder, LocalOptions};
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use types::ed25519::{PrivateKey, PublicKey};
use types::name::Name;

/// The budget of the pool of the transport and the session. Its largest block holds a
/// datagram.
const POOL: block::Config = match block::Config::new(1 << 21) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// The budget of the pool of the frames of the session's reads.
const FRAMES: block::Config = match block::Config::new(64 << 10) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// What a program needs to reach one node as one subject.
#[derive(Clone, Debug)]
pub struct Credentials {
    /// The key of the node, which each hello names as the node it goes through.
    pub node: types::node::Key,
    /// The public key that the node proves in the handshake.
    pub public_key: PublicKey,
    /// The subject that the program acts as.
    pub subject: Name,
    /// A private key that the spec lists for `subject`.
    pub key: PrivateKey,
}

/// A program's session with one node. It can move to and be shared between threads:
/// a thread of its own holds the session. Drop closes the session, and blocks until
/// that thread ends.
#[derive(Debug)]
pub struct Client(Option<Thread>);

/// The thread that holds the session, and the sender whose drop ends it.
#[derive(Debug)]
struct Thread {
    end: oneshot::Sender<()>,
    handle: JoinHandle<()>,
}

impl Client {
    /// Dials the node at `address` over UDP, and blocks until the node admits the
    /// first hello of `credentials.subject`. The client then renews the hello until it
    /// drops.
    ///
    /// # Errors
    ///
    /// [`Error::Thread`] when the OS cannot start the client's thread,
    /// [`Error::Bind`] when the local UDP socket cannot bind, and [`Error::Session`]
    /// when the dial, the handshake, or the hello fails.
    ///
    /// # Panics
    ///
    /// Resumes a panic of the client's thread.
    pub fn connect(
        address: SocketAddr,
        credentials: Credentials,
    ) -> Result<Self, Error> {
        let (ready, connected) = oneshot::channel();
        let (end, ended) = oneshot::channel();
        #[expect(
            clippy::disallowed_methods,
            reason = "the sdk is the env of its thread"
        )]
        let handle = thread::Builder::new()
            .name("foundation-sdk".into())
            .spawn(move || run(address, credentials, ready, ended))
            .map_err(|e| Error::Thread {
                reason: e.to_string(),
            })?;
        let thread = Thread { end, handle };
        match connected.blocking_recv() {
            Ok(Ok(())) => Ok(Self(Some(thread))),
            Ok(Err(error)) => {
                thread.end();
                Err(error)
            }
            Err(_) => {
                thread.end();
                unreachable!("the thread ended with no answer and no panic")
            }
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(thread) = self.0.take() {
            thread.end();
        }
    }
}

impl Thread {
    /// Ends the thread and waits for it, and resumes its panic.
    fn end(self) {
        drop(self.end);
        if let Err(payload) = self.handle.join()
            && !thread::panicking()
        {
            panic::resume_unwind(payload);
        }
    }
}

/// Why [`Client::connect`] gave no client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The OS cannot start the client's thread.
    Thread {
        /// The OS error.
        reason: String,
    },
    /// The local UDP socket cannot bind.
    Bind(transport::Error),
    /// The session failed before the node admitted the hello.
    Session(program::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Thread { reason } => {
                write!(f, "cannot start the client thread: {reason}")
            }
            Self::Bind(error) => write!(f, "cannot bind the local UDP socket: {error}"),
            Self::Session(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for Error {}

/// The body of the client's thread: connects, sends the outcome on `ready`, and holds
/// the session until `ended` completes.
fn run(
    address: SocketAddr,
    credentials: Credentials,
    ready: oneshot::Sender<Result<(), Error>>,
    ended: oneshot::Receiver<()>,
) {
    let runtime = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build_local(LocalOptions::default());
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(e) => {
            let reason = e.to_string();
            drop(ready.send(Err(Error::Thread { reason })));
            return;
        }
    };
    runtime.block_on(async move {
        let spawned = Rc::new(Spawned::default());
        let tasks = Tasks::new(Spawner(Rc::clone(&spawned)));
        match spawned.guard(connect(address, credentials, tasks)).await {
            Ok(session) => {
                if ready.send(Ok(())).is_ok() {
                    drop(spawned.guard(ended).await);
                }
                drop(session);
            }
            Err(error) => drop(ready.send(Err(error))),
        }
    });
}

/// A session with the node at `address`, on the env of the calling thread.
async fn connect(
    address: SocketAddr,
    credentials: Credentials,
    tasks: Tasks,
) -> Result<program::Session, Error> {
    let clock = env::clock::Clock::new(host::Clock::new());
    let entropy = env::entropy::Entropy::new(host::Entropy);
    let pool = Rc::new(block::Pool::heap(POOL));
    let transport = transport::Client::new(transport::client::Config {
        net: env::net::Net::new(host::Net),
        clock: clock.clone(),
        entropy: entropy.clone(),
        tasks: tasks.clone(),
        pool: Rc::clone(&pool),
    })
    .map_err(Error::Bind)?;
    let Credentials {
        node,
        public_key,
        subject,
        key,
    } = credentials;
    let config = program::Config {
        via: node,
        node: public_key,
        addresses: vec![transport::Address::Udp(address)],
        subject,
        key,
        clock,
        entropy,
        tasks,
        pool,
        frames: Rc::new(block::Pool::heap(FRAMES)),
    };
    program::Session::connect(&transport, config)
        .await
        .map_err(Error::Session)
}

/// The tasks of the client's thread. A panic in one ends the thread, as the contract
/// of `env::tasks` asks: Tokio would keep the panic in its join handle.
struct Spawned {
    set: RefCell<JoinSet<()>>,
    /// Wakes the future of [`Spawned::guard`] when a task starts.
    waker: Cell<Waker>,
}

impl Default for Spawned {
    fn default() -> Self {
        Self {
            set: RefCell::default(),
            waker: Cell::new(Waker::noop().clone()),
        }
    }
}

impl Spawned {
    /// Runs `body`, and resumes the first panic of a task while it runs.
    async fn guard<T>(&self, body: impl Future<Output = T>) -> T {
        let mut body = pin!(body);
        poll_fn(|cx| {
            self.check(cx);
            body.as_mut().poll(cx)
        })
        .await
    }

    /// Joins each ended task, and resumes the panic of one that panicked.
    fn check(&self, cx: &mut Context<'_>) {
        self.waker.set(cx.waker().clone());
        while let Poll::Ready(Some(joined)) = self.set.borrow_mut().poll_join_next(cx) {
            if let Err(error) = joined
                && error.is_panic()
            {
                panic::resume_unwind(error.into_panic());
            }
        }
    }
}

/// Spawns the tasks of the client's thread.
struct Spawner(Rc<Spawned>);

impl env::tasks::Driver for Spawner {
    fn spawn(&self, task: Task) {
        self.0.set.borrow_mut().spawn_local(task);
        self.0.waker.replace(Waker::noop().clone()).wake();
    }
}

#[cfg(test)]
mod tests;
