//! How a program on the node's host reaches the node that runs on a data directory.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::rc::Rc;

use env::files::Files;
use wire::hub::client::Refusal;

use crate::{address, admin};

/// The budget of the pool of the transport and the `program::Session`. Its largest
/// block holds a datagram, and the session sends a body in chunks of its largest block.
const POOL: block::Config = match block::Config::new(1 << 21) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// The budget of the pool of the frames of the session's reads. No command reads yet,
/// so it is small. Its largest block is the largest frame that a read takes: a larger
/// one gives `program::Error::Pool`.
const FRAMES: block::Config = match block::Config::new(64 << 10) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// What [`reach`] dials with: the files of the data directory, and the seams of the
/// program's transport and of its `program::Session`.
#[derive(Debug)]
pub struct Config {
    /// The files of the data directory, which `reach` only reads.
    pub files: Files,
    /// The network where the program's transport binds its socket.
    pub net: env::net::Net,
    /// The monotonic clock of the transport and of the renewal of the hello.
    pub clock: env::clock::Clock,
    /// The source of the random values of the transport and of the connection key.
    pub entropy: env::entropy::Entropy,
    /// Where the transport and the `program::Session` spawn their tasks.
    pub tasks: env::tasks::Tasks,
}

/// A `program::Session` with the node that runs on the data directory of `config`, as
/// its first admin. It reads `address` and `admin.key`, and makes and locks nothing. It
/// dials the loopback IP of the family in place of an unspecified IP in `address`.
///
/// # Errors
///
/// Each [`Error`], by its doc.
pub async fn reach(config: Config) -> Result<program::Session, Error> {
    let Config {
        files,
        net,
        clock,
        entropy,
        tasks,
    } = config;
    let address = address::read(&files).await.map_err(Error::Files)?;
    let address = address.ok_or(Error::NotRunning)?;
    let key = admin::read(&files).await.map_err(Error::Files)?;
    let key = key.ok_or(Error::Key)?;
    let pool = Rc::new(block::Pool::heap(POOL));
    let transport = transport::Client::new(transport::client::Config {
        net,
        clock: clock.clone(),
        entropy: entropy.clone(),
        tasks: tasks.clone(),
        pool: Rc::clone(&pool),
    })
    .map_err(Error::Transport)?;
    let at = dialed(address.bound);
    let config = program::Config {
        via: address.key,
        node: address.public_key,
        addresses: vec![transport::Address::Udp(at)],
        subject: spec::founding::admin(),
        key,
        clock,
        entropy,
        tasks,
        pool,
        frames: Rc::new(block::Pool::heap(FRAMES)),
    };
    program::Session::connect(&transport, config)
        .await
        .map_err(|error| match error {
            program::Error::Refused(refusal) => Error::Refused(refusal),
            program::Error::Transport(error @ transport::Error::Network { .. }) => {
                Error::Transport(error)
            }
            program::Error::Transport(error @ transport::Error::Unreachable { .. }) => {
                Error::Unanswered { at, error }
            }
            error => Error::Session { at, error },
        })
}

/// Why [`reach`] gave no session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No node runs on the directory: its file `address` holds no address of a node.
    NotRunning,
    /// The directory holds no file `admin.key` that a node wrote.
    Key,
    /// A file call on the directory failed.
    Files(env::files::Error),
    /// The transport of the program did not start, or its socket broke.
    Transport(transport::Error),
    /// No node answers at `at`, where the file `address` says the node is: the dial
    /// gave no session.
    Unanswered {
        /// The address that the program dialed.
        at: SocketAddr,
        /// Why the dial gave no session.
        error: transport::Error,
    },
    /// The node at `at` took the session, then the session or the hello failed with
    /// `error`.
    Session {
        /// The address that the program dialed.
        at: SocketAddr,
        /// Why `program::Session::connect` gave no session.
        error: program::Error,
    },
    /// The node refused the hello that the key in `admin.key` signs.
    Refused(Refusal),
}

/// Where a program on the node's host dials `bound`: the loopback IP of its family in
/// place of the unspecified IP.
fn dialed(mut bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            bound.set_ip(Ipv4Addr::LOCALHOST.into());
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            bound.set_ip(Ipv6Addr::LOCALHOST.into());
        }
        IpAddr::V4(_) | IpAddr::V6(_) => {}
    }
    bound
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    // A heap pool under loom needs a loom model.
    #[test]
    #[cfg(not(loom))]
    fn the_pool_holds_a_datagram() {
        let pool = block::Pool::heap(POOL);
        assert!(pool.largest() >= 1472, "{}", pool.largest());
    }

    proptest! {
        #[test]
        fn dialed_keeps_each_specified_ip(
            ip in any::<IpAddr>().prop_filter("specified", |ip| !ip.is_unspecified()),
            port: u16,
        ) {
            let bound = SocketAddr::new(ip, port);
            prop_assert_eq!(dialed(bound), bound);
        }

        #[test]
        fn dialed_gives_the_loopback_ip_of_the_family_for_an_unspecified_ip(port: u16) {
            let v4 = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
            let v6 = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port);
            let loopback = |ip: IpAddr| SocketAddr::new(ip, port);
            prop_assert_eq!(dialed(v4), loopback(Ipv4Addr::LOCALHOST.into()));
            prop_assert_eq!(dialed(v6), loopback(Ipv6Addr::LOCALHOST.into()));
        }
    }
}
