//! Writes a reader's samples to InfluxDB.

#![deny(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::string_slice
)]

#[cfg_attr(
    not(feature = "sim"),
    expect(unreachable_pub, reason = "only the sim feature exports it")
)]
mod batch;
#[cfg(feature = "sim")]
#[doc(hidden)]
pub mod bench;
pub mod gap;
pub mod line;
mod send;
#[cfg(feature = "sim")]
pub mod sim;

use std::future;

use connector::kind::{self, Channels, Context, Error};
use connector::{cancel, reader};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Document, read};
use http::Uri;

const BAD_ADDRESS: Code = Code::new("influx.bad-address");
const BAD_DATABASE: Code = Code::new("influx.bad-database");
const BAD_MEASUREMENT: Code = Code::new("influx.bad-measurement");
/// The count of the samples that InfluxDB confirmed with a 204, across the runs of
/// one supervisor call.
const CONFIRMED: &str = "confirmed";

/// The `influx` kind: writes a reader's samples to InfluxDB.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Kind;

/// One influx connector's config.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// The InfluxDB endpoint: a URI that [`connector::http::uri`] takes, with a path
    /// of `/` or none, and no query.
    pub address: Uri,
    /// The InfluxDB 1 database it writes to. Not empty.
    pub database: String,
    /// The measurement of each line: a name that [`line::Measurement::new`] takes,
    /// other than [`gap::MEASUREMENT`].
    pub measurement: String,
    /// The reader whose samples it writes.
    pub reader: reader::Settings,
}

impl kind::Kind for Kind {
    type Config = Config;

    /// Reads `address`, `database`, `measurement`, the settings of [`reader::read`],
    /// and no other key.
    fn parse(&self, config: &Document) -> Result<Config, Vec<Diagnostic>> {
        let reader = reader::read(config, &["address", "database", "measurement"], &[]);
        let address = read::required(
            config,
            kind::NOUN,
            None,
            "address",
            address,
            "Add an `address` attribute with the InfluxDB endpoint, such as \
             \"http://influx:8086\""
                .into(),
        );
        let database = read::required(
            config,
            kind::NOUN,
            None,
            "database",
            database,
            "Add a `database` attribute with the InfluxDB database, such as \"plant\""
                .into(),
        );
        let measurement = read::required(
            config,
            kind::NOUN,
            None,
            "measurement",
            measurement,
            "Add a `measurement` attribute with the name of each line, such as \
             \"telemetry\""
                .into(),
        );
        match (reader, address, database, measurement) {
            (Ok(reader), Ok(address), Ok(database), Ok(measurement)) => Ok(Config {
                address,
                database,
                measurement,
                reader,
            }),
            (reader, address, database, measurement) => {
                let mut diagnostics = reader.err().unwrap_or_default();
                diagnostics.extend(address.err());
                diagnostics.extend(database.err());
                diagnostics.extend(measurement.err());
                Err(diagnostics)
            }
        }
    }

    /// Reads the channels of its reader settings, writes none, and names the count
    /// `confirmed`: the samples that InfluxDB took with a 204.
    #[expect(clippy::unwrap_in_result, reason = "`CONFIRMED` is a valid name")]
    fn check(&self, config: &Config) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels {
            reader: Some(config.reader.clone()),
            counts: vec![CONFIRMED.parse().expect("invariant: `CONFIRMED` is a name")],
            ..Channels::default()
        })
    }

    /// An InfluxDB has no devices to find.
    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        future::ready(Ok(Vec::new()))
    }

    /// Writes the samples of its reader to InfluxDB 1, one line for each sample time
    /// with a field for each channel, keyed by the channel's name. It keeps one
    /// request in flight, and acks a batch only after InfluxDB answers 204, when it
    /// adds the batch's fields to the count `confirmed`. It shows each other answer as
    /// its error and sends the batch again after a backoff.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `influx.sample-type` for a channel that does not hold
    /// `f64` samples, and with `influx.bad-channel` for a channel name that is not a
    /// valid field key. The error of [`hub::reader::Ended`] when the reader ends.
    fn run(&self, ctx: Context<Config>) -> impl Future<Output = Result<(), Error>> {
        send::run(ctx)
    }
}

/// Reads an address: a URI that [`connector::http::uri`] takes, with a path of `/`
/// or none, and no query.
fn address(value: &Value) -> Result<Uri, Diagnostic> {
    let uri = connector::http::uri(value)?;
    let part = match (uri.path(), uri.query()) {
        (_, Some(_)) => "a query",
        ("/", None) => return Ok(uri),
        (_, None) => "a path",
    };
    Err(Diagnostic::new(
        BAD_ADDRESS,
        value.span,
        format!("the address has {part}, which the influx kind does not take"),
        "Remove it, as in \"http://influx:8086\"".into(),
    ))
}

/// Reads a database name: a string that is not empty.
fn database(value: &Value) -> Result<String, Diagnostic> {
    let bad = |message: String| {
        Diagnostic::new(
            BAD_DATABASE,
            value.span,
            message,
            "Write the name of an InfluxDB database, such as \"plant\"".into(),
        )
    };
    match &value.kind {
        value::Kind::String(text) if text.is_empty() => {
            Err(bad("the database name is empty".into()))
        }
        value::Kind::String(text) => Ok(text.to_string()),
        kind => Err(bad(format!("a database is a string, not {}", kind.noun()))),
    }
}

/// Reads a measurement name: a string that [`line::Measurement::new`] takes, other
/// than [`gap::MEASUREMENT`].
fn measurement(value: &Value) -> Result<String, Diagnostic> {
    let bad = |message: String| {
        Diagnostic::new(
            BAD_MEASUREMENT,
            value.span,
            message,
            "Write a measurement name, such as \"telemetry\"".into(),
        )
    };
    let value::Kind::String(text) = &value.kind else {
        return Err(bad(format!(
            "a measurement is a string, not {}",
            value.kind.noun()
        )));
    };
    if **text == *gap::MEASUREMENT {
        return Err(bad(format!("{text:?} is the measurement of the gap lines")));
    }
    line::Measurement::new(text, &[]).map_err(|error| bad(error.to_string()))?;
    Ok(text.to_string())
}

#[cfg(test)]
mod tests;
