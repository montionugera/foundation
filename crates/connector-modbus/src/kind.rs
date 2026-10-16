//! The `modbus` kind: reads the tables of a Modbus TCP device at a fixed rate, and
//! writes one frame per tick.

use std::future;

use connector::cancel;
use connector::kind::{self, Channels, Context, Error};
use document::Document;
use document::diagnostic::Diagnostic;

use crate::config::{self, Config};

#[cfg(test)]
mod tests;

/// The counts of the status of each connector: the samples that the home applied,
/// the ticks that the timer skipped, and the samples that a frame lost.
const COUNTS: [&str; 3] = ["in", "missed", "lost"];

/// The `modbus` kind: reads the tables of a Modbus TCP device at a fixed rate, and
/// writes one frame per tick.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Kind;

impl kind::Kind for Kind {
    type Config = Config;

    /// Reads `address`, `rate`, the optional `unit` and `order`, and one or more
    /// `read` blocks, each with the channel as its label, a `table`, and an
    /// `address`, and no other key or block.
    fn parse(&self, config: &Document) -> Result<Config, Vec<Diagnostic>> {
        config::parse(config)
    }

    /// Gives the channel of each `read` block as a write, in order, and the counts
    /// `in`, `missed`, and `lost`.
    fn check(&self, config: &Config) -> Result<Channels, Vec<Diagnostic>> {
        let counts = COUNTS.map(|count| count.parse().expect("a count is a name"));
        Ok(Channels {
            reader: None,
            writes: config
                .reads
                .iter()
                .map(|read| read.channel.clone())
                .collect(),
            counts: counts.into(),
        })
    }

    /// Finds nothing: the kind does not scan a network.
    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        future::ready(Ok(Vec::new()))
    }

    #[expect(clippy::todo, reason = "dark: `node` has no table line for the kind")]
    async fn run(&self, _: Context<Config>) -> Result<(), Error> {
        todo!("waits on PR B2 of #432")
    }
}
