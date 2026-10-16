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
#[cfg_attr(
    not(feature = "sim"),
    expect(unreachable_pub, reason = "only the sim feature exports it")
)]
mod send;
#[cfg(feature = "sim")]
pub mod sim;

use std::future;

use connector::kind::{self, Channels, Context, Error};
use connector::{cancel, reader, secret};
use document::diagnostic::{Code, Diagnostic};
use document::value::{self, Value};
use document::{Document, read};
use http::Uri;

const BAD_ADDRESS: Code = Code::new("influx.bad-address");
const BAD_BUCKET: Code = Code::new("influx.bad-bucket");
const BAD_DATABASE: Code = Code::new("influx.bad-database");
const BAD_MEASUREMENT: Code = Code::new("influx.bad-measurement");
const BAD_ORG: Code = Code::new("influx.bad-org");
const ORG_WITH_DATABASE: Code = Code::new("influx.org-with-database");
const TWO_TARGETS: Code = Code::new("influx.two-targets");
/// The keys of the config, other than the reader settings.
const KEYS: [&str; 6] = [
    "address",
    "database",
    "bucket",
    "org",
    "measurement",
    "token",
];
/// The count of the samples that InfluxDB confirmed with a 204, across the runs of
/// one supervisor call.
const CONFIRMED: &str = "confirmed";
/// The count of the seqs that the gap lines count, which InfluxDB took with a 204,
/// across the runs of one supervisor call.
const LOST: &str = "lost";

/// The `influx` kind: writes a reader's samples to InfluxDB.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct Kind;

/// One influx connector's config.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// The InfluxDB endpoint: a URI that [`connector::http::uri`] takes, with the
    /// scheme `http`, a path of `/` or none, and no query.
    pub address: Uri,
    /// Where it writes.
    pub target: Target,
    /// The measurement of each line: a name that [`line::Measurement::new`] takes,
    /// other than [`gap::MEASUREMENT`].
    pub measurement: String,
    /// Where the InfluxDB token comes from, sent as `Authorization: Token <value>`
    /// on each request. `None` sends no `Authorization` header. `http` sends it in
    /// clear until TLS.
    pub token: Option<secret::Source>,
    /// The reader whose samples it writes.
    pub reader: reader::Settings,
}

/// Where a connector writes. Each name has 1 to 4,096 bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Target {
    /// The database of InfluxDB 1, written with `POST /write?db=<database>`.
    Database(String),
    /// The bucket of InfluxDB 2, or the database of InfluxDB 3, in `org`, written
    /// with `POST /api/v2/write?org=<org>&bucket=<name>`. InfluxDB 3 takes any `org`.
    Bucket {
        /// The bucket, or the InfluxDB 3 database.
        name: String,
        /// The organization.
        org: String,
    },
}

impl kind::Kind for Kind {
    type Config = Config;

    /// Reads `address`, `database` or `bucket` and `org`, `measurement`, an optional
    /// `token` that [`secret::Source::read`] takes, the settings of
    /// [`reader::read`], and no other key.
    fn parse(&self, config: &Document) -> Result<Config, Vec<Diagnostic>> {
        let reader = reader::read(config, &KEYS, &[]);
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
        let target = target(config);
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
        let token = config.attributes.get("token");
        let token = token
            .map(|token| secret::Source::read(&token.value))
            .transpose();
        match (reader, address, target, measurement, token) {
            (Ok(reader), Ok(address), Ok(target), Ok(measurement), Ok(token)) => {
                Ok(Config {
                    address,
                    target,
                    measurement,
                    token,
                    reader,
                })
            }
            (reader, address, target, measurement, token) => {
                let mut diagnostics = reader.err().unwrap_or_default();
                diagnostics.extend(address.err());
                diagnostics.extend(target.err().into_iter().flatten());
                diagnostics.extend(measurement.err());
                diagnostics.extend(token.err());
                Err(diagnostics)
            }
        }
    }

    /// Reads the channels of its reader settings, writes none, and names two counts:
    /// `confirmed`, the samples that InfluxDB took with a 204, and `lost`, the seqs
    /// that the gap lines of this connector count, which InfluxDB took with a 204.
    fn check(&self, config: &Config) -> Result<Channels, Vec<Diagnostic>> {
        let count = |text: &str| text.parse().expect("invariant: a count is a name");
        Ok(Channels {
            reader: Some(config.reader.clone()),
            counts: vec![count(CONFIRMED), count(LOST)],
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

    /// Writes the samples of its reader to InfluxDB, one line for each sample time with
    /// a field for each channel, keyed by the channel's name. Before the lines of a
    /// frame after seqs that the reader did not get, it writes their `live` gap line
    /// ([`gap::MEASUREMENT`]) at the frame's first sample, also when it leaves that
    /// sample's line out. It keeps one request in flight, and acks a batch only after
    /// InfluxDB answers 204, when it adds the batch's fields to the count `confirmed`
    /// and the seqs of its gap lines to `lost`. It shows each other answer as its error
    /// and sends the batch again after a backoff. It reads the token at its start, and
    /// again after a 401 or a 403 before the next attempt.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] with `influx.sample-type` for a channel that does not hold
    /// `f64` samples, and with `influx.bad-channel` for a channel name that is not a
    /// valid field key. The error of [`hub::reader::Ended`] when the reader ends.
    /// [`Error::Retry`] or [`Error::Config`] from [`Context::secret`] when a read
    /// fails, and [`Error::Retry`] when the token holds a byte that an HTTP header
    /// cannot hold.
    fn run(&self, ctx: Context<Config>) -> impl Future<Output = Result<(), Error>> {
        send::run(ctx)
    }
}

/// Reads an address: a URI that [`connector::http::uri`] takes, with the scheme
/// `http`, a path of `/` or none, and no query.
fn address(value: &Value) -> Result<Uri, Diagnostic> {
    let uri = connector::http::uri(value)?;
    // `https` ships dark until the kind takes a CA (#2702).
    if uri.scheme() == Some(&http::uri::Scheme::HTTPS) {
        return Err(Diagnostic::new(
            BAD_ADDRESS,
            value.span,
            "the scheme of the URI is not http".into(),
            "Write an `http` URI such as \"http://10.0.0.2:8086\"".into(),
        ));
    }
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

/// Reads `database`, or `bucket` and `org`.
fn target(config: &Document) -> Result<Target, Vec<Diagnostic>> {
    let get = |key| config.attributes.get(key);
    match (get("database"), get("bucket"), get("org")) {
        (Some(_), Some(bucket), _) => Err(vec![Diagnostic::new(
            TWO_TARGETS,
            bucket.key_span,
            "the connector has a `database` and a `bucket`".into(),
            "Keep `database` for InfluxDB 1, or `bucket` and `org` for InfluxDB 2 and 3"
                .into(),
        )]),
        (Some(_), None, Some(org)) => Err(vec![Diagnostic::new(
            ORG_WITH_DATABASE,
            org.key_span,
            "the connector has an `org`, which only a `bucket` takes".into(),
            "Remove it, or write a `bucket` in place of the `database`".into(),
        )]),
        (Some(database), None, None) => {
            let database = name(&database.value, BAD_DATABASE, "database");
            database.map(Target::Database).map_err(|error| vec![error])
        }
        (None, None, _) => Err(vec![read::missing(
            kind::NOUN,
            None,
            &["database", "bucket"],
            "Add a `bucket` attribute and an `org` attribute, such as \"plant\", or a \
             `database` attribute for InfluxDB 1"
                .into(),
        )]),
        (None, Some(bucket), _) => {
            let bucket = name(&bucket.value, BAD_BUCKET, "bucket");
            let org = read::required(
                config,
                kind::NOUN,
                None,
                "org",
                |value| name(value, BAD_ORG, "org"),
                "Add an `org` attribute with the InfluxDB organization, such as \
                 \"plant\". InfluxDB 3 takes any name"
                    .into(),
            );
            match (bucket, org) {
                (Ok(name), Ok(org)) => Ok(Target::Bucket { name, org }),
                (bucket, org) => {
                    Err(bucket.err().into_iter().chain(org.err()).collect())
                }
            }
        }
    }
}

/// Reads the name of an InfluxDB `noun`, such as a database: a string of 1 to
/// [`send::NAME_MAX`] bytes.
fn name(value: &Value, code: Code, noun: &str) -> Result<String, Diagnostic> {
    let bad = |message: String| {
        Diagnostic::new(
            code,
            value.span,
            message,
            format!("Write the name of an InfluxDB {noun}, such as \"plant\""),
        )
    };
    match &value.kind {
        value::Kind::String(text) if text.is_empty() => {
            Err(bad(format!("the {noun} name is empty")))
        }
        value::Kind::String(text) if text.len() > send::NAME_MAX => Err(bad(format!(
            "the {noun} name has {} bytes, over {}",
            text.len(),
            send::NAME_MAX
        ))),
        value::Kind::String(text) => Ok(text.to_string()),
        kind => Err(bad(format!("`{noun}` is a string, not {}", kind.noun()))),
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
