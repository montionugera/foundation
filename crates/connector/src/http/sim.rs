//! The HTTP/1.1 server of the protocol simulators of HTTP connectors.

use std::convert::Infallible;
use std::future::poll_fn;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use bytes::Bytes;
use env::net::{self, Listener};
use env::tasks::Tasks;
use http::header::CONNECTION;
use http::{HeaderValue, Request, Response, StatusCode};
use http_body::Body as _;
use hyper::body::Incoming;
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

/// A certificate that [`serve_tls`] gives. The test CA of [`ca`] signs each one
/// other than [`Certificate::OtherCa`]. Each is valid from 2026-01-01 to 2027-01-01
/// UTC, other than [`Certificate::Expired`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Certificate {
    /// For the host `influx`.
    Valid,
    /// For the host `other`.
    OtherHost,
    /// For the host `influx`, valid from 2026-01-01 to 2026-02-01 UTC.
    Expired,
    /// For the host `influx`, signed by a CA that [`ca`] does not hold.
    OtherCa,
}

/// Roots that hold only the test CA, which is valid from 2025-01-01 to 2036-01-01
/// UTC.
///
/// # Panics
///
/// Never: the CA is a fixed file that parses.
#[must_use]
pub fn ca() -> Roots {
    Roots::default()
        .with_pem(include_str!("sim/tls/ca.pem"))
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
/// It runs until the listener fails, and returns that error. Dropping the future stops
/// only the accepts: each stream it accepted runs on.
pub async fn serve(
    listener: Listener,
    tasks: Tasks,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> net::Error {
    run(listener, tasks, None, answer).await
}

/// As [`serve`], under TLS 1.2 or 1.3 with `certificate`. A stream whose handshake
/// fails ends with no answer. It sends no session ticket.
pub async fn serve_tls(
    listener: Listener,
    tasks: Tasks,
    certificate: Certificate,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> net::Error {
    run(listener, tasks, Some(server(certificate)), answer).await
}

pub(super) fn server(certificate: Certificate) -> Arc<ServerConfig> {
    let (pem, key) = match certificate {
        Certificate::Valid => (
            include_str!("sim/tls/influx.pem"),
            include_str!("sim/tls/influx.key"),
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
    };
    let chain = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("invariant: the test certificates parse");
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes())
        .expect("invariant: the test keys parse");
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    // The server checks no client certificate, so it reads no time.
    #[expect(clippy::disallowed_methods, reason = "it passes a fixed time")]
    let builder = ServerConfig::builder_with_details(
        provider,
        Arc::new(At(Stamp::from_nanos(0))),
    );
    let mut config = builder
        .with_safe_default_protocol_versions()
        .expect("invariant: aws-lc-rs has suites of each default version")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("invariant: each test key matches its certificate");
    config.send_tls13_tickets = 0;
    Arc::new(config)
}

async fn run(
    mut listener: Listener,
    tasks: Tasks,
    tls: Option<Arc<ServerConfig>>,
    answer: impl Fn(Request<Bytes>) -> Response<Bytes> + 'static,
) -> net::Error {
    let answer = Rc::new(answer);
    loop {
        let tcp = match poll_fn(|cx| listener.poll_accept(cx)).await {
            Ok(tcp) => tcp,
            Err(error) => return error,
        };
        let answer = Rc::clone(&answer);
        let service = service_fn(move |request| respond(request, Rc::clone(&answer)));
        let session = tls.as_ref().map(|config| {
            let session = rustls::ServerConnection::new(Arc::clone(config)).expect(
                "invariant: a server config with a certificate makes a session",
            );
            Box::new(session.into())
        });
        let mut stream = Stream {
            tcp,
            tls: session,
            received: Rc::default(),
        };
        // A stream that fails has no one to tell.
        tasks.spawn(async move {
            if stream.handshake().await.is_err() {
                return;
            }
            // The person approved this server on the condition that it gets no timer.
            let served = http1::Builder::new()
                .auto_date_header(false)
                .header_read_timeout(None)
                .half_close(true)
                .serve_connection(stream, service);
            drop(served.await);
        });
    }
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
