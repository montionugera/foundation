//! The answers to the name lookups of a run.

use std::net::IpAddr;

use env::net::host::Host;
use types::time::Span;

/// How a lookup of one name goes. Build it with `..Config::default()`: fields get
/// added.
///
/// ```
/// let historian = "10.0.0.2".parse().expect("an address");
/// let config = sim::name::Config {
///     answer: sim::name::Answer::Addresses(vec![historian]),
///     ..sim::name::Config::default()
/// };
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// What the lookup gives.
    pub answer: Answer,
    /// The time that the lookup takes on the clock of its node. Not negative. Zero,
    /// the default, answers at the first poll. A lookup whose end is past the end of
    /// the clock never answers.
    pub delay: Span,
}

/// What a lookup gives.
///
/// ```
/// let lost = sim::name::Answer::Failed;
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// These addresses, in order, each with the port of the lookup. With none, the
    /// default, the lookup gives [`env::net::Error::NotFound`].
    Addresses(Vec<IpAddr>),
    /// [`env::net::Error::Io`] with the Linux `EAGAIN` (11), as when no name server
    /// answers.
    Failed,
}

impl Config {
    /// Panics when the delay is negative.
    pub(crate) fn check(&self, host: &str) {
        let delay = self.delay;
        assert!(
            delay >= Span::ZERO,
            "the lookup of {host} takes a negative delay of {delay}"
        );
    }
}

impl Default for Answer {
    fn default() -> Self {
        Self::Addresses(Vec::new())
    }
}

/// The name of `host`, as [`Host`] parses it.
///
/// Panics when `host` is not a name of [`Host`].
pub(crate) fn parse(host: &str) -> String {
    let parsed: Host = host
        .parse()
        .unwrap_or_else(|e| panic!("{host:?} is not a host: it has {e}"));
    let name = parsed
        .name()
        .unwrap_or_else(|| panic!("{host} is an IP address, which no lookup reads"));
    name.to_owned()
}

/// The key of `name`, a name of [`Host`], in the name table: `name` with no final
/// dot.
pub(crate) fn key(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_owned()
}
