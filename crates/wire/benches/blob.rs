//! The cost of the blob heads that come once per chunk: a put and a reply.

use divan::Bencher;
use types::digest::Digest;
use wire::blob::{Error, FromRequester, Put, Reply, Requester, Server};

const CHUNK_BYTES_MAX: usize = 1 << 20;

fn main() {
    divan::main();
}

/// [`Server::decode`], called out of line as production calls it, so that no bench
/// measures the inlining of its one call site.
#[inline(never)]
fn decode_at_server<'m>(
    server: &Server,
    message: &'m [u8],
) -> Result<FromRequester<'m>, Error> {
    server.decode(message)
}

/// [`Requester::decode`], called out of line for the same reason.
#[inline(never)]
fn decode_at_requester(requester: &Requester, message: &[u8]) -> Result<Reply, Error> {
    requester.decode(message)
}

#[divan::bench]
fn decode_a_put(bencher: Bencher<'_, '_>) {
    let mut put = [0; Put::LEN];
    Put {
        digest: Digest([7; 32]),
        len: 64,
    }
    .encode(&mut put);
    let server = Server::new(CHUNK_BYTES_MAX);
    bencher.bench_local(|| match decode_at_server(&server, divan::black_box(&put)) {
        Ok(FromRequester::Put(put)) => put,
        other => panic!("a put decodes as a put: {other:?}"),
    });
}

#[divan::bench(args = ["chunk", "stored"])]
fn decode_a_reply(bencher: Bencher<'_, '_>, kind: &str) {
    let digest = Digest([7; 32]);
    let reply = match kind {
        "chunk" => Reply::Chunk { digest, len: 64 },
        _ => Reply::Stored { digest },
    };
    let mut message = vec![0; reply.encoded_len()];
    reply.encode(&mut message);
    let requester = Requester::new(CHUNK_BYTES_MAX);
    bencher.bench_local(|| {
        decode_at_requester(&requester, divan::black_box(&message))
            .expect("the reply decodes")
    });
}
