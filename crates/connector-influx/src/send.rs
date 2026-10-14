//! The loop that sends the lines of a reader's frames to InfluxDB.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::future::{self, Future as _, poll_fn};
use std::pin::pin;
use std::task::Poll;

use bytes::Bytes;
use connector::http::Client;
use connector::kind::{Context, Error};
use connector::retry::{self, Backoff};
use http::header::{AUTHORIZATION, HeaderValue};
use http::{Request, StatusCode, Uri};
use hub::reader::{Ended, Position, Received};
use types::time::Span;

use crate::batch::{Batch, Taken};
use crate::{CONFIRMED, Config, Target};

/// The longest name of a database, bucket, or org, in bytes.
pub(crate) const NAME_MAX: usize = 4_096;
const _: () = assert!(
    "/api/v2/write?org=&bucket=&precision=ns".len() + 2 * 3 * NAME_MAX <= 65_534,
    "the longest write path, two names with each byte percent-encoded, fits in the \
     65,534 bytes that `http` takes"
);
/// The longest a write request may take.
const TIMEOUT: Span = Span::from_nanos(10_000_000_000);
/// The largest answer body read, for the text of an error.
const BODY_MAX: usize = 1 << 16;
/// The waits between the attempts of one batch.
const RETRY: retry::Config = retry::Config {
    first: Span::SECOND,
    cap: Span::MINUTE,
};

enum Event<'a> {
    Cancelled,
    /// The position after the batch that InfluxDB took and its samples, or the error
    /// of a read of the token.
    Sent(Result<(Position, u64), Error>),
    Frame(Result<Received<'a>, Ended>),
}

/// Writes the frames of the run's reader to InfluxDB until the cancel, with one
/// request in flight. While one is in flight it reads the next batch, up to 5,000
/// lines, and acks a batch and counts its samples only after InfluxDB answers 204.
pub(crate) async fn run(mut ctx: Context<Config>) -> Result<(), Error> {
    let mut reader = ctx.reader().expect("invariant: check gives a reader");
    let ctx = &ctx;
    let config = ctx.config();
    let client = Client::new(connector::http::Config {
        net: ctx.net().clone(),
        clock: ctx.clock().clone(),
        tasks: ctx.tasks().clone(),
        timeout: TIMEOUT,
        body_max: BODY_MAX,
    });
    let uri = write_uri(&config.address, &config.target);
    let token = RefCell::new(Token::first(ctx).await?);
    let confirmed = ctx.count(CONFIRMED);
    let mut batch = Batch::new(&config.measurement);
    let mut cancelled = pin!(ctx.cancel().wait());
    let mut flight = pin!(None);
    loop {
        if flight.is_none()
            && let Some(Taken {
                body,
                position,
                samples,
            }) = batch.take()
        {
            if body.is_empty() {
                reader.ack(position);
            } else {
                let (client, uri, token) = (&client, &uri, &token);
                flight.set(Some(async move {
                    send(client, ctx, uri, token, body.into()).await?;
                    Ok((position, samples))
                }));
            }
        }
        let full = batch.full();
        let event = {
            let mut frame = pin!(async {
                if full {
                    future::pending().await
                } else {
                    reader.next().await
                }
            });
            poll_fn(|cx| {
                if cancelled.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Event::Cancelled);
                }
                if let Some(sending) = flight.as_mut().as_pin_mut()
                    && let Poll::Ready(sent) = sending.poll(cx)
                {
                    return Poll::Ready(Event::Sent(sent));
                }
                frame.as_mut().poll(cx).map(Event::Frame)
            })
            .await
        };
        match event {
            Event::Cancelled => return Ok(()),
            Event::Sent(Err(error)) => return Err(error),
            Event::Sent(Ok((position, samples))) => {
                flight.set(None);
                reader.ack(position);
                confirmed.add(samples);
            }
            Event::Frame(Ok(received)) => batch.add(&received)?,
            Event::Frame(Err(ended)) => return Err(ended.into()),
        }
    }
}

/// Sends `body` until InfluxDB answers 204. Shows each other answer as the run's
/// error and waits a backoff before the next attempt. After a 401 or a 403, it reads
/// the token again before that attempt. Never ends after the cancel of the run, which
/// the caller sees first.
///
/// # Errors
///
/// The error of [`Token::read`] when a read of the token fails.
async fn send(
    client: &Client,
    ctx: &Context<Config>,
    uri: &Uri,
    token: &RefCell<Option<Token>>,
    body: Bytes,
) -> Result<(), Error> {
    let mut backoff = None;
    loop {
        let request = Request::post(uri.clone()).body(body.clone());
        let mut request = request.expect("invariant: a URI and a body make a request");
        if let Some(token) = &*token.borrow() {
            request.headers_mut().insert(AUTHORIZATION, token.header());
        }
        let refused = match client.send(request).await {
            Ok(answer) if answer.status() == StatusCode::NO_CONTENT => {
                ctx.clear_error();
                return Ok(());
            }
            Ok(answer) => {
                let status = answer.status();
                let text = String::from_utf8_lossy(answer.body());
                let refused = [StatusCode::UNAUTHORIZED, StatusCode::FORBIDDEN]
                    .contains(&status)
                    .then_some(token.borrow())
                    .and_then(|token| Some(token.as_ref()?.source.clone()));
                match &refused {
                    Some(source) => ctx.error(&format_args!(
                        "InfluxDB answered {status}: {}. Check the token {source}",
                        text.trim()
                    )),
                    None => {
                        ctx.error(&format_args!(
                            "InfluxDB answered {status}: {}",
                            text.trim()
                        ));
                    }
                }
                refused
            }
            Err(error) => {
                ctx.error(&format_args!("the write to InfluxDB failed: {error}"));
                None
            }
        };
        let backoff =
            backoff.get_or_insert_with(|| Backoff::new(ctx.clock(), ctx.rng(), RETRY));
        if !backoff.wait(ctx.cancel()).await {
            future::pending::<()>().await;
        }
        if let Some(source) = refused {
            let read = Token::read(ctx, &source).await?;
            *token.borrow_mut() = Some(read);
        }
    }
}

/// The token of a run, as the value of its `Authorization` header.
struct Token {
    source: secret::Source,
    /// `Token <value>`, which a header holds.
    header: secret::Value,
}

impl Token {
    /// Reads the token of the run's config, when it has one.
    ///
    /// # Errors
    ///
    /// As [`Token::read`].
    async fn first(ctx: &Context<Config>) -> Result<Option<Self>, Error> {
        match &ctx.config().token {
            Some(source) => Ok(Some(Self::read(ctx, source).await?)),
            None => Ok(None),
        }
    }

    /// Reads the value of `source`.
    ///
    /// # Errors
    ///
    /// The error of [`Context::secret`], and [`Error::Retry`] when the value holds a
    /// byte that a header cannot hold.
    async fn read(
        ctx: &Context<Config>,
        source: &secret::Source,
    ) -> Result<Self, Error> {
        let value = ctx.secret(source).await?;
        let mut header = Vec::with_capacity(value.expose().len().saturating_add(6));
        header.extend_from_slice(b"Token ");
        header.extend_from_slice(value.expose());
        let header = secret::Value::new(header);
        if HeaderValue::from_bytes(header.expose()).is_err() {
            return Err(Error::Retry(
                format!(
                    "the token {source} holds a byte that an HTTP header cannot hold. \
                     Write the token with no control byte"
                )
                .into(),
            ));
        }
        let source = source.clone();
        Ok(Self { source, header })
    }

    /// The `Authorization` header, marked sensitive.
    fn header(&self) -> HeaderValue {
        let header = HeaderValue::from_bytes(self.header.expose());
        let mut header = header.expect("invariant: `read` checked the bytes");
        header.set_sensitive(true);
        header
    }
}

/// The write URI of `target` at `address`, with nanosecond times.
fn write_uri(address: &Uri, target: &Target) -> Uri {
    let mut path = String::new();
    match target {
        Target::Database(database) => {
            path.push_str("/write?db=");
            encode(&mut path, database);
        }
        Target::Bucket { name, org } => {
            path.push_str("/api/v2/write?org=");
            encode(&mut path, org);
            path.push_str("&bucket=");
            encode(&mut path, name);
        }
    }
    path.push_str("&precision=ns");
    let mut parts = address.clone().into_parts();
    parts.path_and_query = Some(
        path.parse()
            .expect("invariant: names of at most NAME_MAX bytes fit in a query"),
    );
    Uri::from_parts(parts).expect("invariant: an address with a path is a URI")
}

/// Writes `text` into `path`, with each byte other than an unreserved one encoded.
fn encode(path: &mut String, text: &str) {
    for &byte in text.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            path.push(char::from(byte));
        } else {
            write!(path, "%{byte:02X}").expect("invariant: a write to a String works");
        }
    }
}
