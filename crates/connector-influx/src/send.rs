//! The loop that sends the lines of a reader's frames to InfluxDB.

use std::fmt::Write as _;
use std::future::{self, Future as _, poll_fn};
use std::pin::pin;
use std::task::Poll;

use bytes::Bytes;
use connector::http::Client;
use connector::kind::{Context, Error};
use connector::retry::{self, Backoff};
use http::{Request, StatusCode, Uri};
use hub::reader::{Ended, Position, Received};
use types::time::Span;

use crate::Config;
use crate::batch::Batch;

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
    /// The position after the batch that InfluxDB took.
    Sent(Position),
    Frame(Result<Received<'a>, Ended>),
}

/// Writes the frames of the run's reader to InfluxDB until the cancel, with one
/// request in flight. While one is in flight it reads the next batch, up to 5,000
/// lines, and acks a batch only after InfluxDB answers 204.
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
    let uri = write_uri(&config.address, &config.database);
    let mut batch = Batch::new(&config.measurement);
    let mut cancelled = pin!(ctx.cancel().wait());
    let mut flight = pin!(None);
    loop {
        if flight.is_none()
            && let Some((body, position)) = batch.take()
        {
            if body.is_empty() {
                reader.ack(position);
            } else {
                flight.set(Some(send(&client, ctx, &uri, body.into(), position)));
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
            Event::Sent(position) => {
                flight.set(None);
                reader.ack(position);
            }
            Event::Frame(Ok(received)) => batch.add(&received)?,
            Event::Frame(Err(ended)) => return Err(ended.into()),
        }
    }
}

/// Sends `body` until InfluxDB answers 204, and gives `position`. Shows each other
/// answer as the run's error and waits a backoff before the next attempt. Never
/// ends after the cancel of the run, which the caller sees first.
async fn send(
    client: &Client,
    ctx: &Context<Config>,
    uri: &Uri,
    body: Bytes,
    position: Position,
) -> Position {
    let mut backoff = None;
    loop {
        let request = Request::post(uri.clone()).body(body.clone());
        let request = request.expect("invariant: a URI and a body make a request");
        match client.send(request).await {
            Ok(answer) if answer.status() == StatusCode::NO_CONTENT => {
                ctx.clear_error();
                return position;
            }
            Ok(answer) => ctx.error(&format_args!(
                "InfluxDB answered {}: {}",
                answer.status(),
                String::from_utf8_lossy(answer.body()).trim()
            )),
            Err(error) => {
                ctx.error(&format_args!("the write to InfluxDB failed: {error}"));
            }
        }
        let backoff =
            backoff.get_or_insert_with(|| Backoff::new(ctx.clock(), ctx.rng(), RETRY));
        if !backoff.wait(ctx.cancel()).await {
            future::pending::<()>().await;
        }
    }
}

/// The InfluxDB 1 write URI of `database` at `address`, with nanosecond times.
fn write_uri(address: &Uri, database: &str) -> Uri {
    let mut path = String::from("/write?db=");
    for &byte in database.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            path.push(char::from(byte));
        } else {
            write!(path, "%{byte:02X}").expect("invariant: a write to a String works");
        }
    }
    path.push_str("&precision=ns");
    let mut parts = address.clone().into_parts();
    parts.path_and_query = Some(path.parse().expect("invariant: an encoded query"));
    Uri::from_parts(parts).expect("invariant: an address with a path is a URI")
}
