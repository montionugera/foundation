//! The Rust SDK: a program's session with one Foundation node.

mod host;

use std::fmt;
use std::net::SocketAddr;
use std::panic;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use env::tasks::{Group, Tasks};
use host::{Spawned, Spawner};
use tokio::runtime::{Builder, LocalOptions};
use tokio::sync::oneshot;
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

/// The node that a program reaches: where it listens, and the keys that name it.
#[derive(Clone, Debug)]
pub struct Node {
    /// The UDP addresses where the node listens, dialed in this order with a stagger.
    pub addresses: Vec<SocketAddr>,
    /// The key of the node, which each hello names as the node it goes through.
    pub key: types::node::Key,
    /// The public key that the node proves in the handshake.
    pub public_key: PublicKey,
}

/// What a program acts as on a node.
#[derive(Clone, Debug)]
pub struct Subject {
    /// The name of the subject.
    pub name: Name,
    /// A private key that the spec lists for the subject.
    pub private_key: PrivateKey,
}

/// A program's session with one node. It can move to and be shared between threads:
/// a thread of its own holds the session. Drop ends the session and does not wait:
/// that thread then sends the close and ends. [`Client::close`] waits for both. A
/// panic of the thread after a drop goes to the panic hook of the process.
#[derive(Debug)]
pub struct Client(Thread);

/// The thread that holds the session, and the sender whose drop ends it. A drop
/// detaches the thread.
#[derive(Debug)]
struct Thread {
    end: oneshot::Sender<()>,
    handle: JoinHandle<Result<(), Error>>,
}

impl Client {
    /// Dials `node` over UDP, and blocks until it admits the first hello of
    /// `subject`. The client then renews the hello until it closes or drops.
    ///
    /// # Errors
    ///
    /// [`Error::Thread`] when the OS cannot start the client's thread,
    /// [`Error::Bind`] when the local UDP socket cannot bind, and [`Error::Session`]
    /// when the dial, the handshake, or the hello fails. A `node` with no address is
    /// unreachable with no attempt.
    ///
    /// # Panics
    ///
    /// Resumes a panic of the client's thread.
    pub fn connect(node: &Node, subject: Subject) -> Result<Self, Error> {
        let node = node.clone();
        let (ready, connected) = mpsc::channel();
        let (end, ended) = oneshot::channel();
        #[expect(
            clippy::disallowed_methods,
            reason = "the sdk is the env of its thread"
        )]
        let handle = thread::Builder::new()
            .name("foundation-sdk".into())
            .spawn(move || run(node, subject, ready, ended))
            .map_err(|e| Error::Thread {
                reason: e.to_string(),
            })?;
        let thread = Thread { end, handle };
        #[expect(
            clippy::disallowed_methods,
            reason = "connect blocks its caller, which may run a Tokio runtime"
        )]
        let outcome = connected.recv();
        match outcome {
            Ok(Ok(())) => Ok(Self(thread)),
            Ok(Err(error)) => {
                drop(thread.end());
                Err(error)
            }
            Err(_) => {
                drop(thread.end());
                unreachable!("the thread ended with no answer and no panic")
            }
        }
    }
}

impl Client {
    /// Closes the session, and blocks until the close goes out and the client's
    /// thread ends.
    ///
    /// # Errors
    ///
    /// [`Error::Session`] with the cause when the session ended before the close.
    ///
    /// # Panics
    ///
    /// Resumes a panic of the client's thread.
    pub fn close(self) -> Result<(), Error> {
        self.0.end()
    }
}

impl Thread {
    /// Ends the thread and waits for it, and gives its outcome. Resumes its panic.
    fn end(self) -> Result<(), Error> {
        drop(self.end);
        match self.handle.join() {
            Ok(outcome) => outcome,
            Err(payload) if !thread::panicking() => panic::resume_unwind(payload),
            Err(_) => Ok(()),
        }
    }
}

/// Why [`Client::connect`] gave no client, or why the session ended before
/// [`Client::close`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The OS cannot start the client's thread, or the Tokio runtime of that thread.
    Thread {
        /// The error of the OS or of Tokio.
        reason: String,
    },
    /// The local UDP socket cannot bind.
    Bind(transport::Error),
    /// The session failed before the node admitted the hello, or ended before the
    /// close.
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
/// the session until `ended` completes. It then waits for each task, so that the
/// close of the session goes out. Gives the error that ended the session before
/// `ended`.
fn run(
    node: Node,
    subject: Subject,
    ready: mpsc::Sender<Result<(), Error>>,
    ended: oneshot::Receiver<()>,
) -> Result<(), Error> {
    let runtime = Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build_local(LocalOptions::default());
    let runtime = match runtime {
        Ok(runtime) => runtime,
        Err(e) => {
            let reason = e.to_string();
            drop(ready.send(Err(Error::Thread { reason })));
            return Ok(());
        }
    };
    runtime.block_on(async move {
        let spawned = Rc::new(Spawned::default());
        let group = Group::new(Tasks::new(Spawner(Rc::clone(&spawned))));
        let tasks = group.tasks().clone();
        let outcome = match spawned.guard(connect(node, subject, tasks)).await {
            Ok(session) => {
                if ready.send(Ok(())).is_ok() {
                    drop(spawned.guard(ended).await);
                }
                session
                    .ended()
                    .map_or(Ok(()), |error| Err(Error::Session(error)))
            }
            Err(error) => {
                drop(ready.send(Err(error)));
                Ok(())
            }
        };
        spawned.guard(group.ended()).await;
        outcome
    })
}

/// A session with `node` as `subject`, on the env of the calling thread.
async fn connect(
    node: Node,
    subject: Subject,
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
    let config = program::Config {
        via: node.key,
        node: node.public_key,
        addresses: node
            .addresses
            .into_iter()
            .map(transport::Address::Udp)
            .collect(),
        subject: subject.name,
        key: subject.private_key,
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

#[cfg(test)]
mod tests;
