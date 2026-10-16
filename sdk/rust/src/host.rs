//! The env of the client's thread, on the host OS through Tokio.

mod udp;

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::panic;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use env::net::{Connect, Error, Resolve, listener, tcp};
use env::tasks::Task;
use tokio::task::JoinSet;
use tokio::time::Sleep;
use types::time::Monotonic;

/// A monotonic clock from its own epoch, on `Instant`.
///
/// It keeps two rules of `env::clock::Driver` only in part. On Linux and macOS,
/// `Instant` stops while the host sleeps, so a timer fires late by the time asleep. A
/// read has no fence, so the CPU can move it past a nearby load or store. #2932 closes
/// both gaps.
#[derive(Clone, Copy)]
pub(crate) struct Clock {
    epoch: Instant,
}

impl Clock {
    /// A clock that reads 0 now.
    #[expect(clippy::disallowed_methods, reason = "the sdk reads the OS clock")]
    pub(crate) fn new() -> Self {
        Self {
            epoch: Instant::now(),
        }
    }
}

impl env::clock::Driver for Clock {
    #[expect(clippy::disallowed_methods, reason = "the sdk reads the OS clock")]
    fn now(&self) -> Monotonic {
        let nanos = self.epoch.elapsed().as_nanos();
        Monotonic(
            u64::try_from(nanos).expect("the client runs for less than 584 years"),
        )
    }

    fn epoch(&self) -> Instant {
        self.epoch
    }

    fn timer(&self) -> Pin<Box<dyn env::clock::Timer>> {
        Box::pin(Timer {
            clock: *self,
            sleep: Box::pin(tokio::time::sleep_until(self.epoch.into())),
        })
    }
}

/// A Tokio sleep, armed again for each new deadline.
struct Timer {
    clock: Clock,
    sleep: Pin<Box<Sleep>>,
}

impl env::clock::Timer for Timer {
    fn poll_until(
        self: Pin<&mut Self>,
        deadline: Monotonic,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        let this = self.get_mut();
        // A Tokio sleep stays pending until the time driver turns.
        if env::clock::Driver::now(&this.clock) >= deadline {
            return Poll::Ready(());
        }
        let at = (this.clock.epoch + Duration::from_nanos(deadline.0)).into();
        if this.sleep.deadline() != at {
            this.sleep.as_mut().reset(at);
        }
        // A spent coop budget would make a fired sleep pending.
        Pin::new(&mut tokio::task::coop::unconstrained(this.sleep.as_mut())).poll(cx)
    }
}

/// Random bytes from the OS.
pub(crate) struct Entropy;

impl env::entropy::Driver for Entropy {
    fn fill(&self, bytes: &mut [u8]) {
        if let Err(e) = getrandom::fill(bytes) {
            panic!("the OS gives no random bytes: {e}");
        }
    }
}

/// UDP sockets of the host. The client gives the transport only UDP addresses, so a
/// TCP stream or a name lookup is a defect.
pub(crate) struct Net;

impl env::net::Driver for Net {
    fn udp(
        &self,
        config: &env::net::udp::Config,
    ) -> Result<
        (
            Box<dyn env::net::udp::Driver>,
            Box<dyn env::net::udp::receiver::Driver>,
        ),
        Error,
    > {
        let (socket, receiver) = udp::bind(config)?;
        Ok((Box::new(socket), Box::new(receiver)))
    }

    fn connect<'a>(&'a self, _: &'a tcp::Config) -> Connect<'a> {
        panic!("the sdk dials only UDP")
    }

    fn listen(&self, _: &tcp::Listen) -> Result<Box<dyn listener::Driver>, Error> {
        panic!("the sdk dials only UDP")
    }

    fn resolve<'a>(&'a self, _: &'a str, _: u16) -> Resolve<'a> {
        panic!("the sdk dials only UDP")
    }
}

/// The tasks of the client's thread. A panic in one ends the thread, as the contract
/// of `env::tasks` asks: Tokio would keep the panic in its join handle.
pub(crate) struct Spawned {
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
    pub(crate) async fn guard<T>(&self, body: impl Future<Output = T>) -> T {
        let mut body = pin!(body);
        poll_fn(|cx| {
            self.check(cx);
            body.as_mut().poll(cx)
        })
        .await
    }

    /// Waits until each task ends, and resumes the first panic of a task.
    pub(crate) async fn drain(&self) {
        poll_fn(|cx| {
            self.check(cx);
            if self.set.borrow().is_empty() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
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
pub(crate) struct Spawner(pub(crate) Rc<Spawned>);

impl env::tasks::Driver for Spawner {
    fn spawn(&self, task: Task) {
        self.0.set.borrow_mut().spawn_local(task);
        self.0.waker.replace(Waker::noop().clone()).wake();
    }
}

#[cfg(test)]
mod tests;
