use std::future::poll_fn;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use rustls::ServerConfig;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use ::sim::{Sim, name, node};
use bytes::Bytes;
use env::net::{Listener, tcp};
use env::tasks::Tasks;
use env::thread::Handle;
use http::{Request, Response};
use types::time::{Span, Stamp};

use hyper::rt::{Read as _, ReadBuf, Write as _};

use super::super::stream::Stream;
use super::{Pem, Roots};
use crate::http::sim::{self, Certificate};
use crate::http::{Client, Config, Error};

const PORT: u16 = 8086;
/// The size of a body that takes many writes and reads of the stream.
const LARGE: usize = 1 << 20;
const TIMEOUT: Span = Span::from_nanos(10_000_000_000);
/// 2026-06-01T00:00:00Z, inside the validity of each certificate but the expired one.
const JUNE_2026: Stamp = Stamp::from_nanos(1_780_272_000_000_000_000);
/// 2025-06-01T00:00:00Z, before the validity of each server certificate.
const JUNE_2025: Stamp = Stamp::from_nanos(1_748_736_000_000_000_000);

fn shard(name: &str) -> env::shards::Config {
    env::shards::Config {
        name: name.into(),
        core: Some(0),
    }
}

/// What a body gives, or the error of its send.
type Outcome = Result<String, Error>;

/// What the client does next in [`Network::start`].
enum Step {
    Send(&'static str),
    /// A POST of a body of [`LARGE`] bytes.
    Upload(&'static str),
    Wait(Span),
}

/// A server node at the name `influx` that runs `serve_tls`, and a client node whose
/// mesh time starts at a given wall time. The answer gives the length of the body at
/// `/length`, [`LARGE`] bytes at `/large`, and else the path.
struct Network {
    sim: Sim,
    client: node::Node,
    server: node::Node,
    handles: Vec<Handle>,
}

impl Network {
    fn new(seed: u64, certificate: Certificate, wall: Stamp) -> Self {
        let mut sim = Sim::new(::sim::Config {
            seed,
            steps_max: 10_000_000,
            ..::sim::Config::default()
        });
        let client = sim.node(node::Config {
            wall,
            ..node::Config::default()
        });
        let server = sim.node(node::Config::default());
        let answer = name::Answer::Addresses(vec![server.addresses()[0]]);
        sim.name(
            "influx",
            name::Config {
                answer,
                ..name::Config::default()
            },
        );
        let listen = tcp::Listen {
            local: SocketAddr::new(server.addresses()[0], PORT),
            backlog: 4,
            options: super::super::OPTIONS,
        };
        let listener = server.net().listen(&listen).expect("the port is free");
        let handle = server
            .shards()
            .start(shard("server"), move |tasks| async move {
                let answer = |request: Request<Bytes>| {
                    Response::new(match request.uri().path() {
                        "/length" => Bytes::from(request.body().len().to_string()),
                        "/large" => Bytes::from(vec![b'x'; LARGE]),
                        path => Bytes::from(path.to_owned()),
                    })
                };
                drop(sim::serve_tls(listener, tasks, certificate, answer).await);
                // The streams it accepted end when this shard ends.
                std::future::pending::<()>().await;
            })
            .expect("the shard starts");
        Self {
            sim,
            client,
            server,
            handles: vec![handle],
        }
    }

    /// Starts a server on port `port` of the server node that runs `run` on its
    /// listener.
    fn listen<F>(
        &mut self,
        port: u16,
        run: impl FnOnce(Listener, Tasks) -> F + Send + 'static,
    ) where
        F: Future<Output = ()> + 'static,
    {
        let listen = tcp::Listen {
            local: SocketAddr::new(self.server.addresses()[0], port),
            backlog: 4,
            options: super::super::OPTIONS,
        };
        let listener = self.server.net().listen(&listen).expect("the port is free");
        let handle = self
            .server
            .shards()
            .start(shard("other"), move |tasks| async move {
                run(listener, tasks).await;
                std::future::pending::<()>().await;
            })
            .expect("the shard starts");
        self.handles.push(handle);
    }

    /// Starts one client that trusts `roots` on the client node, which runs `steps`
    /// once the node has mesh time. Gives the outcome of each send, as the sends end.
    fn start(&mut self, roots: Roots, steps: Vec<Step>) -> Arc<Mutex<Vec<Outcome>>> {
        let client = self.client.clone();
        let out = Arc::new(Mutex::new(Vec::new()));
        let slot = Arc::clone(&out);
        let handle = self
            .client
            .shards()
            .start(shard("client"), move |tasks| async move {
                let env = crate::common::env(&client, tasks.clone());
                let (hub, _) = hub::testing::open(env).await;
                let clock = client.clock();
                let http = Client::new(Config {
                    net: client.net(),
                    clock: clock.clone(),
                    tasks,
                    timeout: TIMEOUT,
                    body_max: LARGE,
                    roots,
                    time: hub.time().await,
                });
                for step in steps {
                    match step {
                        Step::Wait(span) => clock.sleep(span).await,
                        step @ (Step::Send(uri) | Step::Upload(uri)) => {
                            let request = match step {
                                Step::Upload(_) => Request::post(uri)
                                    .body(Bytes::from(vec![b'x'; LARGE])),
                                _ => Request::get(uri).body(Bytes::new()),
                            };
                            let request = request.expect("a request");
                            let outcome = http.send(request).await.map(|response| {
                                String::from_utf8(response.into_body().to_vec())
                                    .expect("UTF-8")
                            });
                            slot.lock().expect("no panic under the lock").push(outcome);
                        }
                    }
                }
                drop(http);
                clock.sleep(Span::MINUTE).await;
            })
            .expect("the shard starts");
        self.handles.push(handle);
        out
    }

    /// Limits each way between the nodes to 1 MB/s, so a body of [`LARGE`] bytes
    /// fills the stream.
    fn slow(&mut self) {
        let config = ::sim::link::Config {
            rate: NonZeroU64::new(1_000_000),
            ..::sim::link::Config::default()
        };
        self.sim.link(&self.client, &self.server, config);
        self.sim.link(&self.server, &self.client, config);
    }

    /// Runs `steps` from a client that trusts the test CA, crashes the server after
    /// `crash`, and gives the outcome of each send as text: an error with each of its
    /// sources, outermost first.
    fn crash_after(&mut self, steps: Vec<Step>, crash: Span) -> Vec<String> {
        let out = self.start(sim::ca(), steps);
        self.sim.run_for(crash).expect("the run ends");
        let server = self.server.clone();
        self.sim.crash(&server, ::sim::Crash::Process);
        self.sim.run_for(Span::MINUTE).expect("the run ends");
        let outcomes = std::mem::take(&mut *out.lock().expect("no panic"));
        outcomes
            .into_iter()
            .map(|outcome| outcome.unwrap_or_else(|error| chain(&error)))
            .collect()
    }

    /// Sends a GET of `uri` from a client that trusts `roots`.
    #[expect(clippy::unwrap_in_result, reason = "a test may panic")]
    fn send(&mut self, roots: Roots, uri: &'static str) -> Outcome {
        let out = self.start(roots, vec![Step::Send(uri)]);
        self.sim.run_for(Span::MINUTE).expect("the run ends");
        let mut outcomes = std::mem::take(&mut *out.lock().expect("no panic"));
        assert_eq!(outcomes.len(), 1, "one send gives one outcome");
        outcomes.pop().expect("one outcome")
    }
}

/// The message of `error` and of each of its sources, outermost first.
fn chain(error: &Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        messages.push(cause.to_string());
        source = cause.source();
    }
    messages.join(" <- ")
}

fn failed(outcome: Outcome) -> String {
    outcome.expect_err("the send fails").to_string()
}

/// A server config for the certificate of `influx` that takes only `versions`.
fn server(versions: &[&'static rustls::SupportedProtocolVersion]) -> Arc<ServerConfig> {
    let pem = include_str!("../sim/tls/influx.pem");
    let chain = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .expect("the certificate parses");
    let key = include_str!("../sim/tls/influx.key");
    let key = PrivateKeyDer::from_pem_slice(key.as_bytes()).expect("the key parses");
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    #[expect(clippy::disallowed_methods, reason = "a fixed time")]
    let builder = ServerConfig::builder_with_details(
        provider,
        Arc::new(super::At(Stamp::from_nanos(0))),
    );
    let config = builder
        .with_protocol_versions(versions)
        .expect("aws-lc-rs has suites of each version")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("the key matches");
    Arc::new(config)
}

/// Serves one stream on port 8087 under `config`. It answers one request with "ok",
/// then gives the stream to `then`. Gives the text that `then` returns.
fn serve_one<F>(
    network: &mut Network,
    config: Arc<ServerConfig>,
    then: impl FnOnce(Stream) -> F + Send + 'static,
) -> Arc<Mutex<Option<String>>>
where
    F: Future<Output = String> + 'static,
{
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    network.listen(8087, move |mut listener, _| async move {
        let tcp = poll_fn(|cx| listener.poll_accept(cx))
            .await
            .expect("a stream");
        let session = rustls::ServerConnection::new(config).expect("a session");
        let mut stream = Stream {
            tcp,
            tls: Some(Box::new(session.into())),
            received: Rc::default(),
        };
        stream.handshake().await.expect("the handshake ends");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut bytes = [0; 1024];
            let mut buf = ReadBuf::new(&mut bytes);
            poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, buf.unfilled()))
                .await
                .expect("the head comes");
            head.extend_from_slice(buf.filled());
        }
        let answer = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
        poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, answer))
            .await
            .expect("the answer goes");
        poll_fn(|cx| Pin::new(&mut stream).poll_flush(cx))
            .await
            .expect("the answer goes");
        let text = then(stream).await;
        *slot.lock().expect("no panic under the lock") = Some(text);
    });
    out
}

#[test]
fn exchanges_with_a_server_whose_certificate_the_roots_trust() {
    let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
    let body = network.send(sim::ca(), "https://influx:8086/ping");
    assert_eq!(body.expect("the server answers"), "/ping");
}

#[test]
fn takes_port_443_for_https_with_no_port() {
    let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
    let error = failed(network.send(sim::ca(), "https://influx/ping"));
    let remote = SocketAddr::new(network.server.addresses()[0], 443);
    assert_eq!(
        error,
        format!("the connect failed: {remote} refused the connection")
    );
}

#[test]
fn refuses_a_certificate_for_another_host() {
    let mut network = Network::new(2, Certificate::OtherHost, JUNE_2026);
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8086/ping")),
        "TLS with influx:8086 failed: invalid peer certificate: certificate not \
         valid for name \"influx\"; certificate is only valid for DnsName(\"other\")"
    );
}

#[test]
fn refuses_an_expired_certificate_at_mesh_time() {
    let mut network = Network::new(3, Certificate::Expired, JUNE_2026);
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8086/ping")),
        "TLS with influx:8086 failed: invalid peer certificate: certificate \
         expired: verification time 1780272000 (UNIX), but certificate is not valid \
         after 1769904000 (10368000 seconds ago)"
    );
}

#[test]
fn refuses_a_certificate_that_is_not_valid_yet_at_mesh_time() {
    let mut network = Network::new(4, Certificate::Valid, JUNE_2025);
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8086/ping")),
        "TLS with influx:8086 failed: invalid peer certificate: certificate not \
         valid yet: verification time 1748736000 (UNIX), but certificate is not \
         valid before 1767225600 (18489600 seconds in future)"
    );
}

#[test]
fn refuses_a_certificate_of_a_ca_that_the_roots_do_not_hold() {
    let mut network = Network::new(5, Certificate::OtherCa, JUNE_2026);
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8086/ping")),
        "TLS with influx:8086 failed: invalid peer certificate: UnknownIssuer"
    );
}

#[test]
fn fails_the_handshake_with_a_server_that_does_not_speak_tls() {
    let mut network = Network::new(10, Certificate::Valid, JUNE_2026);
    network.listen(8087, |listener, tasks| async move {
        let answer = |_: Request<Bytes>| Response::new(Bytes::new());
        drop(sim::serve(listener, tasks, answer).await);
    });
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8087/ping")),
        "TLS with influx:8087 failed: received corrupt message of type \
         InvalidContentType"
    );
}

#[test]
fn fails_the_handshake_with_a_server_that_closes_the_stream() {
    let mut network = Network::new(11, Certificate::Valid, JUNE_2026);
    network.listen(8087, |mut listener, _| async move {
        while let Ok(mut tcp) = poll_fn(|cx| listener.poll_accept(cx)).await {
            // A close with unread bytes resets the stream, so it reads the hello.
            let mut hello = [0; 4096];
            drop(poll_fn(|cx| tcp.poll_read(cx, &mut hello)).await);
            drop(poll_fn(|cx| tcp.poll_close(cx)).await);
        }
    });
    assert_eq!(
        failed(network.send(sim::ca(), "https://influx:8087/ping")),
        "TLS with influx:8087 failed: the peer closed the stream in the handshake"
    );
}

#[test]
fn fails_the_handshake_with_a_server_that_resets_the_stream() {
    let mut network = Network::new(12, Certificate::Valid, JUNE_2026);
    network.listen(8087, |mut listener, _| async move {
        while let Ok(tcp) = poll_fn(|cx| listener.poll_accept(cx)).await {
            drop(tcp);
        }
    });
    let remote = SocketAddr::new(network.server.addresses()[0], 8087);
    let error = network
        .send(sim::ca(), "https://influx:8087/ping")
        .expect_err("the server resets the stream");
    assert_eq!(
        error.to_string(),
        format!("TLS with influx:8087 failed: {remote} reset the stream")
    );
    let mut source = std::error::Error::source(&error);
    let mut stream = None;
    while let Some(cause) = source {
        stream = stream.or(cause.downcast_ref::<env::net::Error>());
        source = cause.source();
    }
    assert!(
        stream.is_some(),
        "no source of {error:?} is the `env` error"
    );
}

#[test]
fn fails_a_handshake_whose_first_write_finds_the_stream_reset() {
    let mut network = Network::new(17, Certificate::Valid, JUNE_2026);
    network.listen(8087, |mut listener, _| async move {
        while let Ok(tcp) = poll_fn(|cx| listener.poll_accept(cx)).await {
            drop(tcp);
        }
    });
    let remote = SocketAddr::new(network.server.addresses()[0], 8087);
    let client = network.client.clone();
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    let handle = network
        .client
        .shards()
        .start(shard("client"), move |_| async move {
            let config = tcp::Config {
                remote,
                options: super::super::OPTIONS,
            };
            let tcp = client.net().connect(&config).await.expect("a stream");
            // The reset comes before the hello goes, so the first write fails.
            client.clock().sleep(Span::SECOND).await;
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let config = super::client(&provider, &sim::ca(), JUNE_2026);
            let name = "influx".try_into().expect("a DNS name");
            let session =
                rustls::ClientConnection::new(config, name).expect("a session");
            let mut stream = Stream {
                tcp,
                tls: Some(Box::new(session.into())),
                received: Rc::default(),
            };
            let error = stream.handshake().await.expect_err("the stream is reset");
            *slot.lock().expect("no panic") = Some(error.to_string());
        })
        .expect("the shard starts");
    network.handles.push(handle);
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    let got = out.lock().expect("no panic").take();
    assert_eq!(got, Some(format!("{remote} reset the stream")));
}

#[test]
fn sends_and_reads_bodies_that_fill_the_stream() {
    let mut network = Network::new(13, Certificate::Valid, JUNE_2026);
    network.slow();
    let steps = vec![
        Step::Upload("https://influx:8086/length"),
        Step::Send("https://influx:8086/large"),
    ];
    let texts = network.crash_after(steps, Span::MINUTE);
    assert_eq!(texts, [LARGE.to_string(), "x".repeat(LARGE)]);
}

#[test]
fn fails_an_upload_when_the_server_crashes() {
    let mut network = Network::new(14, Certificate::Valid, JUNE_2026);
    network.slow();
    let steps = vec![Step::Upload("https://influx:8086/length")];
    let texts = network.crash_after(steps, Span::from_nanos(500_000_000));
    assert_eq!(
        texts,
        [
            "the exchange failed: connection error <- connection error <- \
          10.0.0.2:8086 reset the stream"
        ]
    );
}

#[test]
fn fails_a_download_when_the_server_crashes() {
    let mut network = Network::new(15, Certificate::Valid, JUNE_2026);
    network.slow();
    let steps = vec![Step::Send("https://influx:8086/large")];
    let texts = network.crash_after(steps, Span::from_nanos(500_000_000));
    assert_eq!(
        texts,
        [
            "the exchange failed: error reading a body from connection <- error \
          reading a body from connection <- 10.0.0.2:8086 reset the stream"
        ]
    );
}

#[test]
fn fails_a_body_that_ends_with_no_close_notify() {
    let mut network = Network::new(16, Certificate::Valid, JUNE_2026);
    network.listen(8087, |mut listener, _| async move {
        let mut tcp = poll_fn(|cx| listener.poll_accept(cx))
            .await
            .expect("a stream");
        let session = rustls::ServerConnection::new(sim::server(Certificate::Valid))
            .expect("a session");
        let mut stream = Stream {
            tcp,
            tls: Some(Box::new(session.into())),
            received: Rc::default(),
        };
        stream.handshake().await.expect("the handshake ends");
        // A close with unread bytes resets the stream, so it reads the whole head.
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut bytes = [0; 1024];
            let mut buf = ReadBuf::new(&mut bytes);
            poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, buf.unfilled()))
                .await
                .expect("the head comes");
            head.extend_from_slice(buf.filled());
        }
        let answer = b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc";
        poll_fn(|cx| Pin::new(&mut stream).poll_write(cx, answer))
            .await
            .expect("the answer goes");
        tcp = stream.tcp;
        poll_fn(|cx| tcp.poll_close(cx))
            .await
            .expect("the stream closes");
    });
    assert_eq!(
        network
            .send(sim::ca(), "https://influx:8087/ping")
            .map_err(|error| chain(&error)),
        Err(
            "the exchange failed: error reading a body from connection <- error \
             reading a body from connection <- peer closed connection without \
             sending TLS close_notify: \
             https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html\
             #unexpected-eof"
                .to_owned()
        )
    );
}

#[test]
fn fails_https_with_no_roots_before_the_lookup() {
    let mut network = Network::new(6, Certificate::Valid, JUNE_2026);
    // No name gives `absent` an address, so a lookup would fail the connect.
    let outcome = network.send(Roots::default(), "https://absent:8086/ping");
    let error = outcome.expect_err("the send fails");
    assert!(matches!(error, Error::NoRoots), "{error:?}");
    assert_eq!(
        error.to_string(),
        "no CA is trusted: install the CA certificates of the OS, or give `ca`"
    );
}

#[test]
fn reuses_an_https_connection_and_opens_a_new_one_for_http() {
    let mut network = Network::new(7, Certificate::Valid, JUNE_2026);
    let steps = vec![
        Step::Send("https://influx:8086/first"),
        Step::Wait(Span::from_nanos(10_000_000_000)),
        Step::Send("https://influx:8086/second"),
        Step::Send("http://influx:8086/plain"),
    ];
    let out = network.start(sim::ca(), steps);
    network
        .sim
        .run_for(Span::from_nanos(5_000_000_000))
        .expect("the run ends");
    let remote = SocketAddr::new(network.server.addresses()[0], PORT);
    network.server.fail_listener(remote);
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    let outcomes = std::mem::take(&mut *out.lock().expect("no panic"));
    let texts: Vec<_> = outcomes
        .into_iter()
        .map(|outcome| outcome.unwrap_or_else(|error| error.to_string()))
        .collect();
    assert_eq!(
        texts,
        [
            "/first".to_owned(),
            "/second".to_owned(),
            format!("the connect failed: {remote} refused the connection"),
        ]
    );
}

#[test]
fn gives_the_same_run_for_the_same_seed() {
    let run = || {
        let mut network = Network::new(8, Certificate::Valid, JUNE_2026);
        let body = network.send(sim::ca(), "https://influx:8086/ping");
        (body.expect("the server answers"), network.sim.digest())
    };
    assert_eq!(run(), run());
}

#[test]
fn adds_the_certificates_of_a_pem_text_to_the_roots() {
    let ca = include_str!("../sim/tls/ca.pem");
    let other = include_str!("../sim/tls/other-ca.pem");
    let roots = Roots::default().with_pem(ca).expect("the CA parses");
    let both = roots
        .with_pem(&format!("text before\n{other}"))
        .expect("the CA parses");
    for certificate in [Certificate::Valid, Certificate::OtherCa] {
        let mut network = Network::new(9, certificate, JUNE_2026);
        let body = network.send(both.clone(), "https://influx:8086/ping");
        assert_eq!(
            body.expect("the server answers"),
            "/ping",
            "{certificate:?}"
        );
    }
    let mut network = Network::new(9, Certificate::OtherCa, JUNE_2026);
    let error = failed(network.send(roots, "https://influx:8086/ping"));
    assert_eq!(
        error,
        "TLS with influx:8086 failed: invalid peer certificate: UnknownIssuer"
    );
}

#[test]
fn gives_empty_for_a_pem_text_with_no_certificate() {
    let key = include_str!("../sim/tls/influx.key");
    for text in ["", "no PEM here", key] {
        let error = Roots::default().with_pem(text).expect_err("no certificate");
        assert_eq!(error, Pem::Empty, "{text}");
        assert_eq!(error.to_string(), "the PEM text holds no certificate");
    }
}

#[test]
fn gives_invalid_for_a_pem_certificate_that_does_not_parse() {
    let broken = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
    let bad_base64 = "-----BEGIN CERTIFICATE-----\n!!!!\n-----END CERTIFICATE-----\n";
    let ca = include_str!("../sim/tls/ca.pem");
    for text in [broken, bad_base64, &format!("{ca}{broken}")] {
        let error = Roots::default().with_pem(text).expect_err("a broken one");
        assert_eq!(error, Pem::Invalid, "{text}");
        assert_eq!(
            error.to_string(),
            "a certificate of the PEM text does not parse"
        );
    }
}

#[test]
fn skips_and_counts_der_certificates_that_do_not_parse() {
    let ca = include_str!("../sim/tls/ca.pem");
    let der = rustls::pki_types::pem::PemObject::from_pem_slice(ca.as_bytes())
        .map(|der: rustls::pki_types::CertificateDer<'_>| der.to_vec())
        .expect("the CA parses");
    let (roots, skipped) = Roots::from_der([der.as_slice(), b"not DER".as_slice()]);
    assert_eq!(skipped, 1);
    let mut network = Network::new(10, Certificate::Valid, JUNE_2026);
    let body = network.send(roots, "https://influx:8086/ping");
    assert_eq!(body.expect("the server answers"), "/ping");
    let (roots, skipped) = Roots::from_der([b"not DER".as_slice()]);
    assert_eq!(skipped, 1);
    let mut network = Network::new(10, Certificate::Valid, JUNE_2026);
    let error = network.send(roots, "https://influx:8086/ping");
    assert!(matches!(error, Err(Error::NoRoots)), "{error:?}");
}

#[test]
fn exchanges_with_a_server_that_takes_only_tls_1_2() {
    let mut network = Network::new(17, Certificate::Valid, JUNE_2026);
    let out = serve_one(
        &mut network,
        server(&[&rustls::version::TLS12]),
        |_| async { String::new() },
    );
    let body = network.send(sim::ca(), "https://influx:8087/ping");
    assert_eq!(body.map_err(|error| chain(&error)).as_deref(), Ok("ok"));
    assert_eq!(out.lock().expect("no panic").as_deref(), Some(""));
}

#[test]
fn sends_close_notify_when_the_client_closes_a_stream() {
    let mut network = Network::new(18, Certificate::Valid, JUNE_2026);
    let out = serve_one(
        &mut network,
        server(rustls::DEFAULT_VERSIONS),
        |mut stream| async move {
            let mut bytes = [0; 64];
            let mut buf = ReadBuf::new(&mut bytes);
            let read =
                poll_fn(|cx| Pin::new(&mut stream).poll_read(cx, buf.unfilled())).await;
            match read {
                Ok(()) => format!("read {} bytes", buf.filled().len()),
                Err(error) => error.to_string(),
            }
        },
    );
    let body = network.send(sim::ca(), "https://influx:8087/ping");
    assert_eq!(body.map_err(|error| chain(&error)).as_deref(), Ok("ok"));
    assert_eq!(
        out.lock().expect("no panic").as_deref(),
        Some("read 0 bytes")
    );
}
