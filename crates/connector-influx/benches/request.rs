//! The cost to build the request of one attempt, with no token and with a token of 88
//! bytes, for a body of 64 KiB. Run with `cargo bench -p connector-influx --bench
//! request`. Before it times anything, it prints the allocations of each build.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use bytes::Bytes;
use connector::secret;
use connector_influx::bench::{Token, request};
use divan::Bencher;
use http::Uri;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const URI: &str = "http://influx:8086/api/v2/write?org=plant&bucket=edge&precision=ns";

/// A token of the length of one that InfluxDB 2 makes.
fn token() -> Token {
    let value = secret::Value::new(vec![b'k'; 88]);
    Token::new(&value).expect("a token of letters")
}

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn main() {
    let (uri, body, token) = (Uri::from_static(URI), body(), token());
    for (name, token) in [("none", None), ("token", Some(&token))] {
        let ((), allocs) = ALLOCATOR.count(|| drop(request(&uri, &body, token)));
        println!("{name}: {allocs} allocations per request");
    }
    divan::main();
}

/// A body of 64 KiB, which each request shares.
fn body() -> Bytes {
    Bytes::from(vec![b'x'; 1 << 16])
}

#[divan::bench]
fn none(bencher: Bencher<'_, '_>) {
    let (uri, body) = (Uri::from_static(URI), body());
    bencher.bench_local(|| request(divan::black_box(&uri), &body, None));
}

#[divan::bench]
fn with_token(bencher: Bencher<'_, '_>) {
    let (uri, body, token) = (Uri::from_static(URI), body(), token());
    bencher.bench_local(|| request(divan::black_box(&uri), &body, Some(&token)));
}
