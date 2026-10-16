//! The env of the client's thread, on the host OS through Tokio.

mod udp;

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use env::net::{Connect, Error, Resolve, listener, tcp};
use tokio::time::Sleep;
use types::time::Monotonic;

/// A monotonic clock from its own epoch, on `Instant`.
///
/// It keeps two rules of `env::clock::Driver` only in part. On Linux and macOS,
/// `Instant` stops while the host sleeps, so a timer fires late by the time asleep. A
/// read has no fence, so the CPU can move it past a nearby load or store.
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
            epoch: self.epoch,
            sleep: Box::pin(tokio::time::sleep_until(self.epoch.into())),
        })
    }
}

/// A Tokio sleep, armed again for each new deadline.
struct Timer {
    epoch: Instant,
    sleep: Pin<Box<Sleep>>,
}

impl env::clock::Timer for Timer {
    fn poll_until(
        self: Pin<&mut Self>,
        deadline: Monotonic,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        let this = self.get_mut();
        let at = (this.epoch + Duration::from_nanos(deadline.0)).into();
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

#[cfg(test)]
mod tests;
