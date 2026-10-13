//! How a program on the node's host reaches the node that runs on a data directory.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::rc::Rc;

use env::files::Files;
use wire::hub::client::Refusal;

use crate::{address, admin};

/// The pool of the transport and of the hub client. Its largest block holds a
/// datagram, and the client sends a body in chunks of its largest block.
const POOL: block::Config = match block::Config::new(1 << 21) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// What [`reach`] dials with: the files of the data directory, and the seams of the
/// program's transport and of its hub client.
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
    /// Where the transport and the hub client spawn their tasks.
    pub tasks: env::tasks::Tasks,
}

/// A client of the node that runs on the data directory of `config`, as its first
/// admin. It reads `address` and `admin.key`, and makes and locks nothing. It dials
/// the loopback IP of the family in place of an unspecified IP in `address`.
///
/// # Errors
///
/// Each [`Error`], by its doc.
pub async fn reach(config: Config) -> Result<hub::client::Client, Error> {
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
    let config = hub::client::Config {
        via: address.key,
        node: address.public_key,
        addresses: vec![transport::Address::Udp(at)],
        subject: spec::founding::admin(),
        key,
        clock,
        entropy,
        tasks,
        pool,
    };
    hub::client::Client::connect(&transport, config)
        .await
        .map_err(|error| match error {
            hub::client::Error::Refused(refusal) => Error::Refused(refusal),
            hub::client::Error::Transport(error @ transport::Error::Network { .. }) => {
                Error::Transport(error)
            }
            hub::client::Error::Transport(
                error @ transport::Error::Unreachable { .. },
            ) => Error::Unanswered { at, error },
            error => Error::Session { at, error },
        })
}

/// Why [`reach`] gave no client.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
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
        /// Why the hub client gave no client.
        error: hub::client::Error,
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
        assert!(POOL.reservation() <= 1 << 27, "{}", POOL.reservation());
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
