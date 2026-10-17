//! The HTTP/1.1 server of the protocol simulators of HTTP connectors.

use std::convert::Infallible;
use std::future::poll_fn;
use std::pin::{Pin, pin};
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::Bytes;
use env::net::{self, Listener};
use env::tasks::{Group, Tasks};
use http::header::CONNECTION;
use http::{HeaderValue, Request, Response, StatusCode};
use http_body::Body as _;
use hyper::body::Incoming;
use hyper::rt::{Read as _, ReadBuf, Write as _};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use types::time::Stamp;

use super::Roots;
use super::body::Whole;
use super::stream::Stream;
use super::tls::At;
use crate::cancel::Token;

/// A certificate that [`serve_tls`] gives. The test CA of [`ca`] signs each one
/// other than [`Certificate::OtherCa`]. Each is valid from 2026-01-01 to 2027-01-01
/// UTC, other than [`Certificate::Expired`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certificate {
    /// For the host `server`.
    Valid,
    /// For the host `other`.
    OtherHost,
    /// For the host `server`, valid from 2026-01-01 to 2026-02-01 UTC.
    Expired,
    /// For the host `server`, signed by a CA that [`ca`] does not hold.
    OtherCa,
    /// For the IP addresses `10.0.0.2` and `fd00::2`.
    Ip,
}

/// The PEM text of the test CA of [`ca`].
pub const CA: &str = include_str!("sim/tls/ca.pem");

/// The PEM text of the CA that signs [`Certificate::OtherCa`], which [`ca`] does not
/// hold.
pub const OTHER_CA: &str = include_str!("sim/tls/other-ca.pem");

/// Roots that hold only the test CA, which is valid from 2025-01-01 to 2036-01-01
/// UTC.
///
/// # Panics
///
/// Never: the CA is a fixed file that parses.
#[must_use]
pub fn ca() -> Roots {
    Roots::default()
        .with_pem(CA)
        .expect("invariant: the test CA parses")
}

/// Answers HTTP/1.1 requests on each stream that `listener` accepts, each stream on
/// its own task of `tasks`, with keep-alive. `answer` gets each request with its
/// whole body. It sends no `date` header and sets no timer, so no clock value
/// changes what it does. A client that closes its write side after a whole request
/// still gets the answer. A request that breaks HTTP gets 400, or 414 when its URI is
/// too long and 431 when its head is too long, and its stream ends. An HTTP/2 preface
/// ends the stream with no answer.
///
/// It runs until `until` is cancelled or the listener fails. It then drops `listener`
/// and closes each stream it accepted, with a `close_notify` under TLS, then a FIN. An
/// answer in flight can be cut. The close waits on no peer: a shutdown that cannot
/// finish at once, and a stream in its TLS handshake, drop the stream, which resets it.
/// Bytes of the peer that reach a stream after its close reset it, as on a TCP stack,
/// so a request in flight at the stop can end with a reset. On each return, each stream
/// that it accepted has ended. Dropping the future stops only the accepts: each stream
/// it accepted runs on until `until`.
///
/// # Errors
///
/// The error of the listener, when it fails before `serve` stops its accepts.
pub async fn serve(
    listener: Listener,
    tasks: Tasks,
    until: Token,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> Result<(), net::Error> {
    run(listener, tasks, until, None, answer).await
}

/// As [`serve`], under TLS 1.2 or 1.3 with `certificate`. A stream whose handshake
/// fails ends with no answer. It sends no session ticket.
///
/// # Errors
///
/// As [`serve`].
pub async fn serve_tls(
    listener: Listener,
    tasks: Tasks,
    until: Token,
    certificate: Certificate,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> Result<(), net::Error> {
    run(listener, tasks, until, Some(server(certificate)), answer).await
}

pub(super) fn server(certificate: Certificate) -> Arc<ServerConfig> {
    let (pem, key) = match certificate {
        Certificate::Valid => (
            include_str!("sim/tls/server.pem"),
            include_str!("sim/tls/server.key"),
        ),
        Certificate::OtherHost => (
            include_str!("sim/tls/other.pem"),
            include_str!("sim/tls/other.key"),
        ),
        Certificate::Expired => (
            include_str!("sim/tls/expired.pem"),
            include_str!("sim/tls/expired.key"),
        ),
        Certificate::OtherCa => (
            include_str!("sim/tls/stranger.pem"),
            include_str!("sim/tls/stranger.key"),
        ),
        Certificate::Ip => (
            include_str!("sim/tls/ip.pem"),
            include_str!("sim/tls/ip.key"),
        ),
    };
    config(pem, key, rustls::DEFAULT_VERSIONS)
}

/// A server config for the certificate chain `pem` and its `key` that takes only
/// `versions` and sends no session ticket.
///
/// # Panics
///
/// If `pem` or `key` does not parse, or the key does not match.
pub(super) fn config(
    pem: &str,
    key: &str,
    versions: &[&'static rustls::SupportedProtocolVersion],
) -> Arc<ServerConfig> {
    let chain = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("the certificates parse");
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).expect("the key parses");
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    // The server checks no client certificate, so it reads no time.
    #[expect(clippy::disallowed_methods, reason = "it passes a fixed time")]
    let builder = ServerConfig::builder_with_details(
        provider,
        Arc::new(At(Stamp::from_nanos(0))),
    );
    let mut config = builder
        .with_protocol_versions(versions)
        .expect("invariant: aws-lc-rs has suites of each version")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("the key matches its certificate");
    config.send_tls13_tickets = 0;
    Arc::new(config)
}

/// As [`serve`], under TLS with `tls` when it is `Some`.
pub(super) async fn run(
    listener: Listener,
    tasks: Tasks,
    until: Token,
    tls: Option<Arc<ServerConfig>>,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> Result<(), net::Error> {
    let (group, stop) = (Group::new(tasks), until.child());
    let accepted = accept(listener, &group, &until, &stop, tls, answer).await;
    stop.cancel();
    group.ended().await;
    accepted
}

/// Serves each stream that `listener` accepts on its own task of `group`, until
/// `stop`, and accepts until the listener has no stream ready once `until` is
/// cancelled, or until its error, which wins.
async fn accept(
    mut listener: Listener,
    group: &Group,
    until: &Token,
    stop: &Token,
    tls: Option<Arc<ServerConfig>>,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> Result<(), net::Error> {
    let (answer, mut cancelled) = (Rc::new(answer), pin!(until.wait()));
    loop {
        let accepted = poll_fn(|cx| {
            let cancelled = until.cancelled() || cancelled.as_mut().poll(cx).is_ready();
            match listener.poll_accept(cx) {
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Ready(Ok(tcp)) => Poll::Ready(Ok(Some(tcp))),
                Poll::Pending if cancelled => Poll::Ready(Ok(None)),
                Poll::Pending => Poll::Pending,
            }
        });
        let Some(tcp) = accepted.await? else {
            return Ok(());
        };
        let answer = Rc::clone(&answer);
        let service = service_fn(move |request| respond(request, Rc::clone(&answer)));
        let (tls, stop) = (tls.clone(), stop.clone());
        // A stream that fails has no one to tell.
        group.tasks().spawn(async move {
            let stream = match tls {
                Some(config) => match stop.race(Stream::server(tcp, config)).await {
                    Some(Ok(stream)) => stream,
                    Some(Err(_)) | None => return,
                },
                None => Stream::plain(tcp, Rc::default()),
            };
            // The person approved this server on the condition that it gets no timer.
            let mut served = http1::Builder::new()
                .auto_date_header(false)
                .header_read_timeout(None)
                .half_close(true)
                .serve_connection(stream, service);
            if stop.race(&mut served).await.is_none() {
                let mut stream = served.into_parts().io;
                poll_fn(|cx| {
                    close(&mut stream, cx);
                    Poll::Ready(())
                })
                .await;
            }
        });
    }
}

/// Reads `stream` until it has no bytes, then starts its shutdown, with no wait on
/// the peer. A drop of a stream with bytes unread, or before its shutdown is done,
/// resets it.
fn close(stream: &mut Stream, cx: &mut Context<'_>) {
    let mut bytes = [0; 1024];
    loop {
        let mut read = ReadBuf::new(&mut bytes);
        match Pin::new(&mut *stream).poll_read(cx, read.unfilled()) {
            Poll::Ready(Ok(())) if !read.filled().is_empty() => {}
            Poll::Ready(_) | Poll::Pending => break,
        }
    }
    drop(Pin::new(stream).poll_shutdown(cx));
}

async fn respond(
    request: Request<Incoming>,
    answer: Rc<impl Fn(Request<Bytes>) -> Response<Bytes>>,
) -> Result<Response<Whole>, Infallible> {
    let (parts, mut body) = request.into_parts();
    let mut bytes = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        let Ok(frame) = frame else {
            let mut response = reply(StatusCode::BAD_REQUEST, String::new());
            let close = HeaderValue::from_static("close");
            response.headers_mut().insert(CONNECTION, close);
            return Ok(response.map(|body| Whole(Some(body))));
        };
        if let Ok(data) = frame.into_data() {
            bytes.extend_from_slice(&data);
        }
    }
    let response = answer(Request::from_parts(parts, Bytes::from(bytes)));
    Ok(response.map(|body| Whole(Some(body))))
}

fn reply(status: StatusCode, text: String) -> Response<Bytes> {
    let mut response = Response::new(Bytes::from(text));
    *response.status_mut() = status;
    response
}

#[cfg(test)]
#[cfg(not(loom))]
mod tests;
