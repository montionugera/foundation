use std::future::poll_fn;
use std::io::{self, IoSlice};
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, ready};

use proptest::prelude::*;
use rustls::ServerConfig;
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use ::sim::{Sim, name, node};
use bytes::Bytes;
use env::net::host::Host;
use env::net::{Listener, Tcp, tcp};
use env::tasks::Tasks;
use env::thread::Handle;
use http::{Request, Response};
use types::time::{Span, Stamp};

use super::super::stream::Stream;
use super::super::tests::stream_error;
use super::{Pem, Roots};
use crate::http::sim::{self, Certificate};
use crate::http::{Client, Config, Error, Tls};

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

/// A server node at the name `server` that runs `serve_tls`, and a client node whose
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
            "server",
            name::Config {
                answer,
                ..name::Config::default()
            },
        );
        let [v4, v6] = server.addresses().map(|ip| {
            let listen = tcp::Listen {
                local: SocketAddr::new(ip, PORT),
                backlog: 4,
                options: super::super::OPTIONS,
            };
            server.net().listen(&listen).expect("the port is free")
        });
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
                let serve = sim::serve_tls(v6, tasks.clone(), certificate, answer);
                tasks.spawn(async move { drop(serve.await) });
                drop(sim::serve_tls(v4, tasks, certificate, answer).await);
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
                    tls: Some(Tls {
                        roots,
                        time: hub.time().await,
                    }),
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
    /// sources, outermost first, then the source that is the `env` error.
    fn crash_after(&mut self, steps: Vec<Step>, crash: Span) -> Vec<String> {
        let out = self.start(sim::ca(), steps);
        self.sim.run_for(crash).expect("the run ends");
        let server = self.server.clone();
        self.sim.crash(&server, ::sim::Crash::Process);
        self.sim.run_for(Span::MINUTE).expect("the run ends");
        let outcomes = std::mem::take(&mut *out.lock().expect("no panic"));
        outcomes
            .into_iter()
            .map(|outcome| {
                outcome.unwrap_or_else(|error| {
                    let stream = stream_error(&error);
                    format!("{}; the `env` error: {stream:?}", chain(&error))
                })
            })
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

/// The certificate and key of the host `server` that the test CA signs.
const SIGNED: [&str; 2] = [
    include_str!("../sim/tls/server.pem"),
    include_str!("../sim/tls/server.key"),
];
/// A self-signed certificate for `server` with `CA:false`, and its key.
const SELF: [&str; 2] = [
    include_str!("../sim/tls/self.pem"),
    include_str!("../sim/tls/self.key"),
];
/// A self-signed certificate for `server` with `CA:true`, and its key.
const SELF_CA: [&str; 2] = [
    include_str!("../sim/tls/self-ca.pem"),
    include_str!("../sim/tls/self-ca.key"),
];

/// Serves `config` on port 8087 with the sim server, which answers "ok".
fn serve(network: &mut Network, config: Arc<ServerConfig>) {
    network.listen(8087, move |listener, tasks| async move {
        let answer = |_: Request<Bytes>| Response::new(Bytes::from_static(b"ok"));
        drop(sim::run(listener, tasks, Some(config), answer).await);
    });
}

/// The TCP stream of a test server, as Tokio I/O for `tokio-rustls`. A test that
/// reads or closes the TCP stream under a TLS session uses it, since a hyper server
/// hides that stream.
struct Peer(Tcp);

impl AsyncRead for Peer {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let n = ready!(self.get_mut().0.poll_read(cx, buf.initialize_unfilled()))
            .map_err(io::Error::other)?;
        buf.advance(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Peer {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let bufs = [IoSlice::new(buf)];
        self.get_mut()
            .0
            .poll_write(cx, &bufs)
            .map_err(io::Error::other)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().0.poll_close(cx).map_err(io::Error::other)
    }
}

/// A server's TLS session over a [`Peer`].
type Session = TlsStream<Peer>;

/// Accepts one stream on `listener`, runs the server handshake of [`SIGNED`], reads
/// the head of one request, and sends `answer`.
async fn accept(mut listener: Listener, answer: &[u8]) -> Session {
    let tcp = poll_fn(|cx| listener.poll_accept(cx))
        .await
        .expect("a stream");
    let config = sim::config(SIGNED[0], SIGNED[1], rustls::DEFAULT_VERSIONS);
    let mut session = TlsAcceptor::from(config)
        .accept(Peer(tcp))
        .await
        .expect("the handshake ends");
    // A close with unread bytes resets the stream, so it reads the whole head.
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut bytes = [0; 1024];
        let n = read(&mut session, &mut bytes)
            .await
            .expect("the head comes");
        head.extend_from_slice(bytes.get(..n).expect("a read fits its buffer"));
    }
    let mut answer = answer;
    while !answer.is_empty() {
        let n = poll_fn(|cx| Pin::new(&mut session).poll_write(cx, answer))
            .await
            .expect("the answer goes");
        answer = answer.get(n..).expect("a write fits its buffer");
    }
    poll_fn(|cx| Pin::new(&mut session).poll_flush(cx))
        .await
        .expect("the answer goes");
    session
}

/// Reads `session` into `bytes`, and gives the count of bytes read.
async fn read(session: &mut Session, bytes: &mut [u8]) -> io::Result<usize> {
    let mut buf = ReadBuf::new(bytes);
    poll_fn(|cx| Pin::new(&mut *session).poll_read(cx, &mut buf)).await?;
    Ok(buf.filled().len())
}

#[test]
fn exchanges_with_a_server_whose_certificate_the_roots_trust() {
    let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
    let body = network.send(sim::ca(), "https://server:8086/ping");
    assert_eq!(body.expect("the server answers"), "/ping");
}

#[test]
fn exchanges_with_an_ip_host_whose_certificate_names_the_ip() {
    for uri in [
        "https://10.0.0.2:8086/ping",
        "https://[fd00::2]:8086/ping",
        "https://[fd00::2%252]:8086/ping",
    ] {
        let mut network = Network::new(1, Certificate::Ip, JUNE_2026);
        let body = network.send(sim::ca(), uri);
        assert_eq!(body.expect("the server answers"), "/ping", "{uri}");
    }
}

#[test]
fn refuses_an_ip_host_whose_certificate_names_only_a_dns_name() {
    for (uri, origin, ip) in [
        ("https://10.0.0.2:8086/ping", "10.0.0.2:8086", "10.0.0.2"),
        ("https://[fd00::2]:8086/ping", "[fd00::2]:8086", "fd00::2"),
        (
            "https://[fd00::2%252]:8086/ping",
            "[fd00::2%2]:8086",
            "fd00::2",
        ),
    ] {
        let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
        assert_eq!(
            failed(network.send(sim::ca(), uri)),
            format!(
                "TLS with {origin} failed: invalid peer certificate: certificate not \
                 valid for name \"{ip}\"; certificate is only valid for \
                 DnsName(\"server\")"
            ),
            "{uri}"
        );
    }
}

#[test]
fn checks_a_name_with_a_final_dot_as_the_name_with_none() {
    let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
    let body = network.send(sim::ca(), "https://server.:8086/ping");
    assert_eq!(body.expect("the server answers"), "/ping");
}

#[test]
fn takes_port_443_for_https_with_no_port() {
    let mut network = Network::new(1, Certificate::Valid, JUNE_2026);
    let error = failed(network.send(sim::ca(), "https://server/ping"));
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
        failed(network.send(sim::ca(), "https://server:8086/ping")),
        "TLS with server:8086 failed: invalid peer certificate: certificate not \
         valid for name \"server\"; certificate is only valid for DnsName(\"other\")"
    );
}

#[test]
fn refuses_an_expired_certificate_at_mesh_time() {
    let mut network = Network::new(3, Certificate::Expired, JUNE_2026);
    assert_eq!(
        failed(network.send(sim::ca(), "https://server:8086/ping")),
        "TLS with server:8086 failed: invalid peer certificate: certificate \
         expired: verification time 1780272000 (UNIX), but certificate is not valid \
         after 1769904000 (10368000 seconds ago)"
    );
}

#[test]
fn refuses_a_certificate_that_is_not_valid_yet_at_mesh_time() {
    let mut network = Network::new(4, Certificate::Valid, JUNE_2025);
    assert_eq!(
        failed(network.send(sim::ca(), "https://server:8086/ping")),
        "TLS with server:8086 failed: invalid peer certificate: certificate not \
         valid yet: verification time 1748736000 (UNIX), but certificate is not \
         valid before 1767225600 (18489600 seconds in future)"
    );
}

#[test]
fn refuses_a_certificate_of_a_ca_that_the_roots_do_not_hold() {
    let mut network = Network::new(5, Certificate::OtherCa, JUNE_2026);
    assert_eq!(
        failed(network.send(sim::ca(), "https://server:8086/ping")),
        "TLS with server:8086 failed: invalid peer certificate: UnknownIssuer"
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
        failed(network.send(sim::ca(), "https://server:8087/ping")),
        "TLS with server:8087 failed: received corrupt message of type \
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
        failed(network.send(sim::ca(), "https://server:8087/ping")),
        "TLS with server:8087 failed: tls handshake eof"
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
        .send(sim::ca(), "https://server:8087/ping")
        .expect_err("the server resets the stream");
    assert_eq!(
        error.to_string(),
        format!("TLS with server:8087 failed: {remote} reset the stream")
    );
    assert_eq!(
        stream_error(&error),
        Some(format!("{remote} reset the stream"))
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
            let name = "server".try_into().expect("a DNS name");
            let Err(error) = Stream::client(tcp, config, name, Rc::default()).await
            else {
                panic!("the stream is reset")
            };
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
        Step::Upload("https://server:8086/length"),
        Step::Send("https://server:8086/large"),
    ];
    let texts = network.crash_after(steps, Span::MINUTE);
    assert_eq!(texts, [LARGE.to_string(), "x".repeat(LARGE)]);
}

#[test]
fn fails_an_upload_when_the_server_crashes() {
    let mut network = Network::new(14, Certificate::Valid, JUNE_2026);
    network.slow();
    let steps = vec![Step::Upload("https://server:8086/length")];
    let texts = network.crash_after(steps, Span::from_nanos(500_000_000));
    assert_eq!(
        texts,
        [
            "the exchange failed: connection error <- connection error <- \
             10.0.0.2:8086 reset the stream; the `env` error: \
             Some(\"10.0.0.2:8086 reset the stream\")"
        ]
    );
}

#[test]
fn fails_a_download_when_the_server_crashes() {
    let mut network = Network::new(15, Certificate::Valid, JUNE_2026);
    network.slow();
    let steps = vec![Step::Send("https://server:8086/large")];
    let texts = network.crash_after(steps, Span::from_nanos(500_000_000));
    assert_eq!(
        texts,
        [
            "the exchange failed: error reading a body from connection <- error \
             reading a body from connection <- 10.0.0.2:8086 reset the stream; the \
             `env` error: Some(\"10.0.0.2:8086 reset the stream\")"
        ]
    );
}

#[test]
fn fails_a_body_that_ends_with_no_close_notify() {
    let mut network = Network::new(16, Certificate::Valid, JUNE_2026);
    network.listen(8087, |listener, _| async move {
        let answer = b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nabc";
        let mut session = accept(listener, answer).await;
        let tcp = &mut session.get_mut().0.0;
        poll_fn(|cx| tcp.poll_close(cx))
            .await
            .expect("the stream closes");
    });
    assert_eq!(
        network
            .send(sim::ca(), "https://server:8087/ping")
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
        Step::Send("https://server:8086/first"),
        Step::Wait(Span::from_nanos(10_000_000_000)),
        Step::Send("https://server:8086/second"),
        Step::Send("http://server:8086/plain"),
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
        let body = network.send(sim::ca(), "https://server:8086/ping");
        (body.expect("the server answers"), network.sim.digest())
    };
    assert_eq!(run(), run());
}

#[test]
fn adds_the_certificates_of_a_pem_text_to_the_roots() {
    let roots = Roots::default().with_pem(sim::CA).expect("the CA parses");
    let both = roots
        .with_pem(&format!("text before\n{}", sim::OTHER_CA))
        .expect("the CA parses");
    for certificate in [Certificate::Valid, Certificate::OtherCa] {
        let mut network = Network::new(9, certificate, JUNE_2026);
        let body = network.send(both.clone(), "https://server:8086/ping");
        assert_eq!(
            body.expect("the server answers"),
            "/ping",
            "{certificate:?}"
        );
    }
    let mut network = Network::new(9, Certificate::OtherCa, JUNE_2026);
    let error = failed(network.send(roots, "https://server:8086/ping"));
    assert_eq!(
        error,
        "TLS with server:8086 failed: invalid peer certificate: UnknownIssuer"
    );
}

#[test]
fn gives_empty_for_a_pem_text_with_no_certificate() {
    let key = include_str!("../sim/tls/server.key");
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
    for text in [broken, bad_base64, &format!("{}{broken}", sim::CA)] {
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
    let der = rustls::pki_types::pem::PemObject::from_pem_slice(sim::CA.as_bytes())
        .map(|der: rustls::pki_types::CertificateDer<'_>| der.to_vec())
        .expect("the CA parses");
    let (roots, skipped) = Roots::from_der([der.as_slice(), b"not DER".as_slice()]);
    assert_eq!(skipped, 1);
    let mut network = Network::new(10, Certificate::Valid, JUNE_2026);
    let body = network.send(roots, "https://server:8086/ping");
    assert_eq!(body.expect("the server answers"), "/ping");
    let (roots, skipped) = Roots::from_der([b"not DER".as_slice()]);
    assert_eq!(skipped, 1);
    let mut network = Network::new(10, Certificate::Valid, JUNE_2026);
    let error = network.send(roots, "https://server:8086/ping");
    assert!(matches!(error, Err(Error::NoRoots)), "{error:?}");
}

#[test]
fn exchanges_with_a_server_that_takes_only_tls_1_2() {
    let mut network = Network::new(17, Certificate::Valid, JUNE_2026);
    let tls12 = &[&rustls::version::TLS12];
    serve(&mut network, sim::config(SIGNED[0], SIGNED[1], tls12));
    let body = network.send(sim::ca(), "https://server:8087/ping");
    assert_eq!(body.map_err(|error| chain(&error)).as_deref(), Ok("ok"));
}

#[test]
fn sends_close_notify_and_a_close_when_the_client_closes_a_stream() {
    let mut network = Network::new(18, Certificate::Valid, JUNE_2026);
    let out = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&out);
    network.listen(8087, move |listener, _| async move {
        let answer = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";
        let mut session = accept(listener, answer).await;
        let mut bytes = [0; 64];
        let tls = match read(&mut session, &mut bytes).await {
            Ok(n) => format!("read {n} bytes"),
            Err(error) => error.to_string(),
        };
        // A stream that the client drops with no close is a reset.
        let tcp = &mut session.get_mut().0.0;
        let text = match poll_fn(|cx| tcp.poll_read(cx, &mut bytes)).await {
            Ok(n) => format!("{tls}, then TCP read {n} bytes"),
            Err(error) => format!("{tls}, then {error}"),
        };
        *slot.lock().expect("no panic under the lock") = Some(text);
    });
    let body = network.send(sim::ca(), "https://server:8087/ping");
    assert_eq!(body.map_err(|error| chain(&error)).as_deref(), Ok("ok"));
    assert_eq!(
        out.lock().expect("no panic").as_deref(),
        Some("read 0 bytes, then TCP read 0 bytes")
    );
}

#[test]
fn trusts_a_self_signed_certificate_with_ca_false_in_the_roots() {
    let mut network = Network::new(19, Certificate::Valid, JUNE_2026);
    serve(
        &mut network,
        sim::config(SELF[0], SELF[1], rustls::DEFAULT_VERSIONS),
    );
    let roots = Roots::default()
        .with_pem(SELF[0])
        .expect("the certificate parses");
    let body = network.send(roots, "https://server:8087/ping");
    assert_eq!(body.map_err(|error| chain(&error)).as_deref(), Ok("ok"));
}

#[test]
fn tells_how_to_fix_a_self_signed_certificate_with_ca_true() {
    let mut network = Network::new(20, Certificate::Valid, JUNE_2026);
    network.listen(8087, |mut listener, _| async move {
        let tcp = poll_fn(|cx| listener.poll_accept(cx))
            .await
            .expect("a stream");
        let config = sim::config(SELF_CA[0], SELF_CA[1], rustls::DEFAULT_VERSIONS);
        drop(Stream::server(tcp, config).await);
    });
    let roots =
        (Roots::default().with_pem(SELF_CA[0])).expect("the certificate parses");
    assert_eq!(
        network
            .send(roots, "https://server:8087/ping")
            .map_err(|error| chain(&error)),
        Err(
            "TLS with server:8087 failed: the server's certificate is a CA \
             certificate, with CA:TRUE: give the server a certificate with \
             CA:FALSE, or one that this CA signs <- the server's certificate is a \
             CA certificate, with CA:TRUE: give the server a certificate with \
             CA:FALSE, or one that this CA signs <- invalid peer certificate: \
             Other(OtherError(CaUsedAsEndEntity))"
                .to_owned()
        )
    );
}

/// A text in the name grammar of `Host`: parts of 1 to 63 bytes, or a name of 253
/// bytes, each with or with no final dot.
fn host_name() -> impl Strategy<Value = String> {
    const PART: &str = "[a-zA-Z0-9_-]{1,63}";
    let parts = prop::collection::vec(PART, 1..=5);
    let longest = (
        "[a-zA-Z0-9_-]{63}",
        "[a-zA-Z0-9_-]{63}",
        "[a-zA-Z0-9_-]{63}",
    )
        .prop_flat_map(|(a, b, c)| ("[a-zA-Z0-9_-]{61}", Just(vec![a, b, c])))
        .prop_map(|(d, mut parts)| {
            parts.push(d);
            parts
        });
    (prop_oneof![parts, longest], any::<bool>()).prop_map(|(parts, dot)| {
        let name = parts.join(".");
        if dot { name + "." } else { name }
    })
}

proptest! {
    /// It calls `name` itself, since a send of each case would build a simulation for
    /// each.
    #[test]
    fn takes_each_name_of_a_host_as_a_tls_name(text in host_name()) {
        let host = text.parse::<Host>();
        prop_assume!(host.is_ok(), "`Host` refuses {:?}", text);
        let host = host.expect("a host");
        let shown = host.name().expect("a name").trim_end_matches('.').to_owned();
        prop_assert_eq!(
            super::super::name(&host),
            ServerName::try_from(shown).expect("a TLS name")
        );
    }
}

#[test]
fn checks_the_longest_name_with_a_final_dot_as_the_name_with_none() {
    let name = format!("{0}.{0}.{0}.{1}", "a".repeat(63), "b".repeat(61));
    let mut network = Network::new(21, Certificate::Valid, JUNE_2026);
    let answer = name::Answer::Addresses(vec![network.server.addresses()[0]]);
    let config = name::Config {
        answer,
        ..name::Config::default()
    };
    network.sim.name(&name, config);
    let uri = format!("https://{name}.:8086/ping").leak();
    assert_eq!(
        failed(network.send(sim::ca(), uri)),
        format!(
            "TLS with {name}.:8086 failed: invalid peer certificate: certificate not \
             valid for name \"{name}\"; certificate is only valid for \
             DnsName(\"server\")"
        )
    );
}
