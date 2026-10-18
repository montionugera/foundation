use std::future::poll_fn;
use std::io::IoSlice;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use ::sim::{Sim, node};
use bytes::Bytes;
use env::net::{self, tcp};
use env::thread::Handle;
use http::{Method, Request, Response, StatusCode};
use types::time::Span;

use super::{Scheme, serve};
use crate::cancel::Token;
use crate::http::{Client, Config};

const PORT: u16 = 8086;
const TIMEOUT: Span = Span::from_nanos(10_000_000_000);

fn shard(name: &str) -> env::shards::Config {
    env::shards::Config {
        name: name.into(),
        core: Some(0),
    }
}

/// How a stream ended for the client, with the bytes it read.
#[derive(Debug, PartialEq)]
enum End {
    /// The server closed it.
    Closed(String),
    /// The read failed with this error.
    Failed(String),
}

/// What `serve` gave, and the count of the streams that the server node then held.
type Ended = (Result<(), net::Error>, usize);

/// A server node that runs `serve` with an answer that echoes each request, and a
/// client node.
struct Network {
    sim: Sim,
    client: node::Node,
    server: node::Node,
    remote: SocketAddr,
    /// Stops `serve`.
    until: Token,
    /// Drops the future of `serve`, which stops its accepts and leaves its streams.
    accepts: Token,
    /// Set once `serve` ends.
    ended: Arc<Mutex<Option<Ended>>>,
    /// Each request that reached the answer, as the answer echoes it.
    seen: Arc<Mutex<Vec<String>>>,
    handles: Vec<Handle>,
}

/// Echoes `request` as its method, URI, version, and body, and sends back the value
/// of its `x-key` header.
fn echo(request: &Request<Bytes>) -> Response<Bytes> {
    let text = format!(
        "{} {} {:?} {}",
        request.method(),
        request.uri(),
        request.version(),
        String::from_utf8_lossy(request.body()),
    );
    let mut response = Response::new(Bytes::from(text));
    if let Some(key) = request.headers().get("x-key") {
        response.headers_mut().insert("x-key", key.clone());
    }
    response
}

impl Network {
    fn new() -> Self {
        let mut sim = Sim::new(::sim::Config {
            seed: 7,
            steps_max: 10_000_000,
            ..::sim::Config::default()
        });
        let client = sim.node(node::Config::default());
        let server = sim.node(node::Config::default());
        let remote = SocketAddr::new(server.addresses()[0], PORT);
        let listen = tcp::Listen {
            local: remote,
            backlog: 4,
            options: super::super::OPTIONS,
        };
        let listener = server.net().listen(&listen).expect("the port is free");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let slot = Arc::clone(&seen);
        let answer = move |request: Request<Bytes>| {
            let response = echo(&request);
            let text = String::from_utf8(response.body().to_vec()).expect("UTF-8");
            slot.lock().expect("no panic under the lock").push(text);
            response
        };
        let (until, ended) = (Token::new(), Arc::new(Mutex::new(None)));
        let (stop, out, node) = (until.clone(), Arc::clone(&ended), server.clone());
        let accepts = Token::new();
        let dropped = accepts.clone();
        let handle = server
            .shards()
            .start(shard("server"), move |tasks| async move {
                let serve = serve(listener, tasks, stop, Scheme::Http, answer);
                if let Some(end) = dropped.race(serve).await {
                    let held = node.streams();
                    *out.lock().expect("no panic under the lock") = Some((end, held));
                }
                // The streams it accepted end when this shard ends.
                std::future::pending::<()>().await;
            })
            .expect("the shard starts");
        Self {
            sim,
            client,
            server,
            remote,
            until,
            accepts,
            ended,
            seen,
            handles: vec![handle],
        }
    }

    /// Sends `requests` in order on one client, and gives the status, `x-key`
    /// header, and body of each response.
    fn send(
        &mut self,
        requests: Vec<Request<Bytes>>,
    ) -> Vec<(StatusCode, String, String)> {
        let (net, clock) = (self.client.net(), self.client.clock());
        let out = Arc::new(Mutex::new(Vec::new()));
        let slot = Arc::clone(&out);
        let handle = self
            .client
            .shards()
            .start(shard("client"), move |tasks| async move {
                let client = Client::new(Config {
                    net,
                    clock,
                    tasks,
                    timeout: TIMEOUT,
                    body_max: 1024,
                    tls: None,
                });
                for request in requests {
                    let response = client.send(request).await.expect("an answer");
                    let key = response
                        .headers()
                        .get("x-key")
                        .map_or("", |key| key.to_str().expect("a visible value"));
                    let text =
                        String::from_utf8(response.body().to_vec()).expect("UTF-8");
                    slot.lock().expect("no panic under the lock").push((
                        response.status(),
                        key.to_owned(),
                        text,
                    ));
                }
            })
            .expect("the shard starts");
        self.handles.push(handle);
        self.sim.run_for(Span::MINUTE).expect("the run ends");
        std::mem::take(&mut *out.lock().expect("no panic under the lock"))
    }

    /// Writes `request` on a new stream, closes the write side when `half_closed`,
    /// and reads until the server ends the stream.
    fn exchange(&mut self, request: &'static [u8], half_closed: bool) -> End {
        let out = self.client(move |client, config| async move {
            let mut tcp = connect(&client, &config).await;
            write(&mut tcp, request).await.expect("the write works");
            if half_closed {
                poll_fn(|cx| tcp.poll_close(cx))
                    .await
                    .expect("the close works");
            }
            read(&mut tcp).await
        });
        self.sim.run_for(Span::MINUTE).expect("the run ends");
        take(&out).expect("the exchange ends")
    }

    /// Runs `main` on a new client shard with the client node and the config of a
    /// stream to the server. Gives the slot that holds what `main` gives, once it
    /// ends.
    fn client<T, F>(
        &mut self,
        main: impl FnOnce(node::Node, tcp::Config) -> F + Send + 'static,
    ) -> Arc<Mutex<Option<T>>>
    where
        T: Send + 'static,
        F: Future<Output = T> + 'static,
    {
        let (client, remote) = (self.client.clone(), self.remote);
        let out = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&out);
        let handle = self
            .client
            .shards()
            .start(shard("client"), move |_| async move {
                let config = tcp::Config {
                    remote,
                    options: super::super::OPTIONS,
                };
                let end = main(client, config).await;
                *slot.lock().expect("no panic under the lock") = Some(end);
            })
            .expect("the shard starts");
        self.handles.push(handle);
        out
    }

    /// What `serve` gave, once it ended.
    fn ended(&self) -> Option<Ended> {
        take(&self.ended)
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("no panic under the lock").clone()
    }

    fn request(&self, method: Method, path: &str, body: &str) -> Request<Bytes> {
        Request::builder()
            .method(method)
            .uri(format!("http://{}{path}", self.remote))
            .header("x-key", path)
            .body(Bytes::from(body.to_owned()))
            .expect("a valid request")
    }
}

fn take<T>(slot: &Mutex<Option<T>>) -> Option<T> {
    slot.lock().expect("no panic under the lock").take()
}

async fn connect(client: &node::Node, config: &tcp::Config) -> net::Tcp {
    client
        .net()
        .connect(config)
        .await
        .expect("the server listens")
}

/// Writes all of `bytes` on `tcp`.
///
/// # Errors
///
/// The error of the first write that fails.
async fn write(tcp: &mut net::Tcp, bytes: &[u8]) -> Result<(), net::Error> {
    let mut rest = bytes;
    while !rest.is_empty() {
        let n = poll_fn(|cx| tcp.poll_write(cx, &[IoSlice::new(rest)])).await?;
        rest = rest.get(n..).expect("a write fits its buffer");
    }
    Ok(())
}

/// Reads `tcp` until the server ends it.
async fn read(tcp: &mut net::Tcp) -> End {
    let mut read = Vec::new();
    let mut bytes = [0; 1024];
    loop {
        match poll_fn(|cx| tcp.poll_read(cx, &mut bytes)).await {
            Ok(0) => return End::Closed(String::from_utf8(read).expect("UTF-8")),
            Ok(n) => read.extend_from_slice(bytes.get(..n).expect("fits")),
            Err(error) => return End::Failed(error.to_string()),
        }
    }
}

#[test]
fn answers_each_request_of_a_client_with_its_whole_body() {
    let mut network = Network::new();
    let requests = vec![
        network.request(Method::POST, "/write?db=a", "m v=1 1"),
        network.request(Method::GET, "/ping", ""),
    ];
    assert_eq!(
        network.send(requests),
        [
            (
                StatusCode::OK,
                "/write?db=a".into(),
                "POST /write?db=a HTTP/1.1 m v=1 1".into()
            ),
            (StatusCode::OK, "/ping".into(), "GET /ping HTTP/1.1 ".into()),
        ]
    );
}

#[test]
fn answers_and_ends_the_stream_on_connection_close() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\nhost: h\r\n\
        content-length: 3\r\nconnection: close\r\n\r\nabc";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, false),
        End::Closed(
            "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 20\r\n\r\n\
            POST /a HTTP/1.1 abc"
                .into()
        )
    );
}

#[test]
fn answers_a_client_that_closes_its_write_side_after_the_request() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\ncontent-length: 3\r\n\r\nabc";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, true),
        End::Closed(
            "HTTP/1.1 200 OK\r\ncontent-length: 20\r\n\r\nPOST /a HTTP/1.1 abc".into()
        )
    );
}

#[test]
fn answers_each_of_two_requests_in_one_write_in_order() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\ncontent-length: 1\r\n\r\n1\
        POST /b HTTP/1.1\r\ncontent-length: 1\r\nconnection: Close\r\n\r\n2";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, false),
        End::Closed(
            "HTTP/1.1 200 OK\r\ncontent-length: 18\r\n\r\nPOST /a HTTP/1.1 1\
            HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: 18\r\n\r\n\
            POST /b HTTP/1.1 2"
                .into()
        )
    );
}

#[test]
fn answers_an_http_1_0_request_and_ends_the_stream() {
    const REQUEST: &[u8] = b"GET /a HTTP/1.0\r\n\r\n";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, false),
        End::Closed(
            "HTTP/1.0 200 OK\r\ncontent-length: 16\r\n\r\nGET /a HTTP/1.0 ".into()
        )
    );
}

#[test]
fn answers_a_body_that_ends_early_with_400_and_nothing_reaches_answer() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\ncontent-length: 30\r\n\r\nabc";
    let mut network = Network::new();
    assert_eq!(network.exchange(REQUEST, true), End::Closed(REFUSED.into()));
    assert_eq!(network.seen(), [""; 0]);
}

#[test]
fn resets_a_stream_whose_head_ends_early_and_answers_nothing() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\ncontent-len";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, true),
        End::Failed("10.0.0.2:8086 reset the stream".into())
    );
    assert_eq!(network.seen(), [""; 0]);
}

const REFUSED: &str = "HTTP/1.1 400 Bad Request\r\nconnection: close\r\n\
    content-length: 0\r\n\r\n";

#[test]
fn answers_a_request_that_breaks_http_with_400_and_ends_the_stream() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\nbad header\r\n\r\n\
        GET /b HTTP/1.1\r\n\r\n";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, false),
        End::Closed(REFUSED.into())
    );
    assert_eq!(network.seen(), [""; 0]);
}

#[test]
fn answers_a_content_length_that_is_not_one_number_with_400() {
    for request in [
        &b"POST /a HTTP/1.1\r\ncontent-length: +3\r\n\r\nabc"[..],
        b"POST /a HTTP/1.1\r\ncontent-length: 3\r\ncontent-length: 4\r\n\r\nabcd",
    ] {
        let mut network = Network::new();
        assert_eq!(
            network.exchange(request, false),
            End::Closed(REFUSED.into())
        );
        assert_eq!(network.seen(), [""; 0]);
    }
}

#[test]
fn answers_a_chunked_body_that_breaks_http_with_400_and_ends_the_stream() {
    for request in [
        &b"POST /a HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\nzz\r\nabc\r\n0\r\n\r\n"[..],
        b"POST /a HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3\r\nabcXX0\r\n\r\n",
    ] {
        let mut network = Network::new();
        assert_eq!(
            network.exchange(request, false),
            End::Closed(REFUSED.into())
        );
        assert_eq!(network.seen(), [""; 0]);
    }
}

#[test]
fn answers_a_chunked_body_whole() {
    const REQUEST: &[u8] = b"POST /a HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n\
        3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, true),
        End::Closed(
            "HTTP/1.1 200 OK\r\ncontent-length: 22\r\n\r\nPOST /a HTTP/1.1 abcde"
                .into()
        )
    );
    assert_eq!(network.seen(), ["POST /a HTTP/1.1 abcde"]);
}

#[test]
fn answers_a_head_with_too_many_headers_with_431_and_ends_the_stream() {
    let mut request = b"GET /a HTTP/1.1\r\n".to_vec();
    for _ in 0..101 {
        request.extend_from_slice(b"x: 1\r\n");
    }
    request.extend_from_slice(b"\r\n");
    let mut network = Network::new();
    assert_eq!(
        network.exchange(request.leak(), false),
        End::Closed(
            "HTTP/1.1 431 Request Header Fields Too Large\r\nconnection: close\r\n\
             content-length: 0\r\n\r\n"
                .into()
        )
    );
    assert_eq!(network.seen(), [""; 0]);
}

#[test]
fn answers_a_uri_that_is_too_long_with_414_and_ends_the_stream() {
    let request = format!("GET /{} HTTP/1.1\r\n\r\n", "a".repeat(70_000));
    let mut network = Network::new();
    assert_eq!(
        network.exchange(request.into_bytes().leak(), false),
        End::Closed(
            "HTTP/1.1 414 URI Too Long\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
                .into()
        )
    );
    assert_eq!(network.seen(), [""; 0]);
}

#[test]
fn ends_the_stream_of_an_http2_preface_with_no_answer() {
    const REQUEST: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    let mut network = Network::new();
    assert_eq!(
        network.exchange(REQUEST, false),
        End::Failed("10.0.0.2:8086 reset the stream".into())
    );
    assert_eq!(network.seen(), [""; 0]);
}

/// Opens a stream to the server and writes `sent` on it, then runs `stop` 500 ms into
/// the run. Gives what `serve` gave, how the stream ended for the client, how a later
/// connect ended, and the address of the server.
fn stopped(
    stop: fn(&Network),
    sent: &'static [u8],
) -> (Option<Ended>, Option<Peer>, SocketAddr) {
    let mut network = Network::new();
    let out = network.client(move |client, config| async move {
        let mut tcp = connect(&client, &config).await;
        write(&mut tcp, sent).await.expect("the write works");
        let ended = read(&mut tcp).await;
        (ended, client.net().connect(&config).await.map(drop))
    });
    let half = Span::from_nanos(500_000_000);
    network.sim.run_for(half).expect("the run goes on");
    assert_eq!(network.server.streams(), 1);
    stop(&network);
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    (network.ended(), take(&out), network.remote)
}

/// How the stream of the client ended, and how a later connect ended.
type Peer = (End, Result<(), net::Error>);

#[test]
fn a_cancel_closes_each_stream_and_returns_once_they_ended() {
    let (ended, client, remote) = stopped(|network| network.until.cancel(), b"");
    assert_eq!(ended, Some((Ok(()), 0)));
    let refused = Err(net::Error::Refused { remote });
    assert_eq!(client, Some((End::Closed(String::new()), refused)));
}

#[test]
fn a_failed_accept_closes_each_stream_and_gives_its_error_once_they_ended() {
    let fail = |network: &Network| network.server.fail_listener(network.remote);
    let (ended, client, remote) = stopped(fail, b"");
    assert_eq!(ended, Some((Err(net::Error::Io { code: 5 }), 0)));
    let refused = Err(net::Error::Refused { remote });
    assert_eq!(client, Some((End::Closed(String::new()), refused)));
}

#[test]
fn a_cancel_closes_a_stream_in_the_middle_of_a_head() {
    let sent = b"GET /a HTTP/1.1\r\nhost: ser";
    let (ended, client, remote) = stopped(|network| network.until.cancel(), sent);
    assert_eq!(ended, Some((Ok(()), 0)));
    let refused = Err(net::Error::Refused { remote });
    assert_eq!(client, Some((End::Closed(String::new()), refused)));
}

#[test]
fn a_listener_that_fails_and_a_cancel_in_one_step_give_the_error() {
    let mut network = Network::new();
    network.sim.run_for(Span::SECOND).expect("the run goes on");
    network.server.fail_listener(network.remote);
    network.until.cancel();
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    assert_eq!(network.ended(), Some((Err(net::Error::Io { code: 5 }), 0)));
}

#[test]
fn a_dropped_serve_leaves_each_stream_until_the_cancel() {
    let mut network = Network::new();
    let out = network.client(|client, config| async move {
        let mut tcp = connect(&client, &config).await;
        client.clock().sleep(Span::SECOND).await;
        let request = b"GET /a HTTP/1.1\r\nhost: server\r\n\r\n";
        write(&mut tcp, request).await.expect("the write works");
        read(&mut tcp).await
    });
    let half = Span::from_nanos(500_000_000);
    network.sim.run_for(half).expect("the run goes on");
    network.accepts.cancel();
    network
        .sim
        .run_for(Span::from_nanos(1_500_000_000))
        .expect("the run goes on");
    assert_eq!(network.seen(), ["GET /a HTTP/1.1 "]);
    assert_eq!(network.server.streams(), 1);
    network.until.cancel();
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    assert_eq!(network.server.streams(), 0);
    let answer = "HTTP/1.1 200 OK\r\ncontent-length: 16\r\n\r\nGET /a HTTP/1.1 ";
    assert_eq!(take(&out), Some(End::Closed(answer.into())));
    assert_eq!(network.ended(), None);
}

/// Pauses the server from 500 ms for a second, and cancels `until` in the pause. The
/// client connects at once, or in the pause when `late`, and writes `sent` in the
/// pause, then its FIN when `fin`, so the server sees the cancel, the stream, and its
/// bytes in one poll. Gives what `serve` gave and how the stream ended for the client.
fn paused(late: bool, fin: bool, sent: String) -> (Option<Ended>, Option<End>) {
    let mut network = Network::new();
    let out = network.client(move |client, config| async move {
        let in_pause = Span::from_nanos(750_000_000);
        if late {
            client.clock().sleep(in_pause).await;
        }
        let mut tcp = match client.net().connect(&config).await {
            Ok(tcp) => tcp,
            Err(error) => return End::Failed(error.to_string()),
        };
        if !late {
            client.clock().sleep(in_pause).await;
        }
        write(&mut tcp, sent.as_bytes())
            .await
            .expect("the write works");
        if fin {
            poll_fn(|cx| tcp.poll_close(cx))
                .await
                .expect("the close works");
        }
        read(&mut tcp).await
    });
    let half = Span::from_nanos(500_000_000);
    network.sim.run_for(half).expect("the run goes on");
    network.server.pause(Span::SECOND);
    network.sim.run_for(half).expect("the run goes on");
    network.until.cancel();
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    (network.ended(), take(&out))
}

#[test]
fn a_cancel_closes_a_stream_that_the_listener_gives_in_the_same_poll() {
    let (ended, client) = paused(true, true, String::new());
    assert_eq!(ended, Some((Ok(()), 0)));
    assert_eq!(client, Some(End::Closed(String::new())));
}

#[test]
fn a_cancel_closes_a_stream_with_bytes_that_arrive_in_the_same_poll() {
    // More bytes than one read of the close takes.
    let pad = "a".repeat(2000);
    let sent = format!("GET /a HTTP/1.1\r\nhost: server\r\nx-pad: {pad}\r\n\r\n");
    let (ended, client) = paused(false, false, sent);
    assert_eq!(ended, Some((Ok(()), 0)));
    assert_eq!(client, Some(End::Closed(String::new())));
}

#[test]
fn a_cancel_closes_a_stream_with_a_request_that_hyper_did_not_read() {
    let mut network = Network::new();
    let out = network.client(|client, config| async move {
        let mut tcp = connect(&client, &config).await;
        let body = "a".repeat(300_000);
        let head = "POST /a HTTP/1.1\r\nhost: server\r\ncontent-length: 300000\r\n\r\n";
        let first = format!("{head}{body}");
        write(&mut tcp, first.as_bytes())
            .await
            .expect("the write works");
        client.clock().sleep(Span::SECOND).await;
        // The answer to the first request fills the window, so hyper reads none of
        // the second.
        let second = b"GET /b HTTP/1.1\r\nhost: server\r\n\r\n";
        write(&mut tcp, second).await.expect("the write works");
        client.clock().sleep(Span::from_nanos(1_500_000_000)).await;
        read(&mut tcp).await
    });
    network
        .sim
        .run_for(Span::from_nanos(2_000_000_000))
        .expect("the run goes on");
    network.until.cancel();
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    assert_eq!(network.ended(), Some((Ok(()), 0)));
    let read = take(&out);
    let Some(End::Closed(read)) = read else {
        panic!("the server closes the stream, not {read:?}")
    };
    assert_eq!(read.len(), 131_072);
    assert!(read.starts_with("HTTP/1.1 200 OK\r\n"), "{read:.40}");
}

#[test]
fn a_request_that_reaches_a_stream_after_its_close_resets_it() {
    let mut network = Network::new();
    let until = network.until.clone();
    let out = network.client(|client, config| async move {
        let mut tcp = connect(&client, &config).await;
        client.clock().sleep(Span::SECOND).await;
        let request = b"GET /a HTTP/1.1\r\nhost: server\r\n\r\n";
        write(&mut tcp, request).await.expect("the write works");
        until.cancel();
        client.clock().sleep(Span::SECOND).await;
        read(&mut tcp).await
    });
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    assert_eq!(network.ended(), Some((Ok(()), 0)));
    let reset = End::Failed("10.0.0.2:8086 reset the stream".into());
    assert_eq!(take(&out), Some(reset));
}

#[test]
fn an_upload_that_goes_on_after_the_close_resets_the_stream() {
    let mut network = Network::new();
    let until = network.until.clone();
    let out = network.client(|client, config| async move {
        let mut tcp = connect(&client, &config).await;
        let head =
            "POST /a HTTP/1.1\r\nhost: server\r\ncontent-length: 1000000\r\n\r\n";
        write(&mut tcp, head.as_bytes())
            .await
            .expect("the write works");
        let chunk = "a".repeat(100_000);
        write(&mut tcp, chunk.as_bytes())
            .await
            .expect("the write works");
        until.cancel();
        let failed = write(&mut tcp, chunk.as_bytes()).await;
        client.clock().sleep(Span::SECOND).await;
        (
            failed.map_err(|error| error.to_string()),
            read(&mut tcp).await,
        )
    });
    network.sim.run_for(Span::MINUTE).expect("the run ends");
    assert_eq!(network.ended(), Some((Ok(()), 0)));
    let reset = "10.0.0.2:8086 reset the stream";
    assert_eq!(
        take(&out),
        Some((Err(reset.into()), End::Failed(reset.into())))
    );
}
