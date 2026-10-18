use std::cell::Cell;

use super::common::{rebuilt, reorder, too_large, with_id};
use super::{
    FITS, Peer, RELAY, Relay, VALUE, drop_read, fits, mistyped, numbered, relayed, run,
    serve,
};
use crate::client::{Client, READS};
use crate::connection::CHUNKS;
use crate::ffi::Status;
use crate::value::{Fault, Value};

/// The largest message of a client of open62541 by default, in bytes.
const MESSAGE: u32 = 1 << 29;

/// Connects a client of `peer` to `url` that takes messages of up to `message` bytes,
/// and reads [`VALUE`] once, as open62541 reads the namespace array after the connect.
async fn connect(peer: &mut Peer<'_>, url: &str, message: u32) -> Client {
    let mut client = peer.limited(url, None, Some(message)).await;
    assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
    client
}

/// What the relay of [`interleaved`] sends after the held chunks of the dropped read.
#[derive(Clone, Copy, Debug)]
enum End {
    /// The abort of the dropped read.
    Abort,
    /// The rest of the chunks of the dropped read.
    Rest,
}

/// The answers of a dropped read D of [`FITS`] (8 chunks), then of a read R of it and
/// of a read of [`VALUE`], and the value of the server. The relay sends the first `d`
/// intermediate chunks of D, then the first `r` chunks of R, then `between` more
/// intermediate chunks of D, then `end`, and then the rest of R. The client takes
/// messages of up to `message` bytes.
fn interleaved(
    seed: u64,
    [d, r, between]: [usize; 3],
    end: End,
    message: u32,
) -> (Result<Value, Fault>, Result<Value, Fault>, Value) {
    run(seed, async move |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.add_bytes(&FITS, 7 * super::CHUNK);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let armed = Cell::new(false);
            let order = |held: Vec<Vec<u8>>| {
                let (dropped, read) = held.split_at(8);
                let kinds: Vec<_> = held.iter().map(|chunk| chunk[3]).collect();
                assert_eq!(kinds[..8], *b"CCCCCCCF", "the answer of D");
                assert!(kinds[8..].iter().all(|&kind| kind == b'C'), "chunks of R");
                let mut out = dropped[..d].to_vec();
                out.extend_from_slice(read);
                out.extend_from_slice(&dropped[d..d + between]);
                match end {
                    End::Abort => out.push(too_large(&dropped[7])),
                    End::Rest => out.extend_from_slice(&dropped[d + between..]),
                }
                out
            };
            let body = async {
                let mut client = connect(&mut peer, &url, message).await;
                armed.set(true);
                drop_read(&mut client, &FITS).await;
                let read = client.read_one(&FITS).await;
                assert!(!armed.get(), "the relay sent the held chunks");
                let after = client.read_one(&VALUE).await;
                client.close().await;
                (read, after)
            };
            let rewrite = reorder(&armed, 8 + r, order);
            let reads = relayed(&node, &Relay::default(), rewrite, body).await;
            (reads.0, reads.1, peer.value)
        })
        .await
    })
    .0
}

/// The client counts the chunks of each answer against [`CHUNKS`], so 3 chunks of a
/// dropped read and then 6 of another read, 9 in all, leave each its place, with and
/// without the abort of the dropped read.
#[test]
fn the_chunks_of_two_answers_each_count_against_the_bound_of_their_own() {
    for end in [End::Abort, End::Rest] {
        let (read, after, value) = interleaved(0, [3, 6, 0], end, MESSAGE);
        assert_eq!(
            (read, after),
            (mistyped("ByteString"), Ok(value)),
            "{end:?}"
        );
    }
}

/// Each split of the chunks of a dropped read around those of another read leaves the
/// other read its answer.
#[test]
fn each_split_of_the_chunks_of_a_dropped_read_leaves_the_other_read_its_answer() {
    let splits = [
        [1, 7, 0],
        [2, 6, 0],
        [0, 7, 1],
        [1, 5, 1],
        [1, 1, 0],
        [1, 1, 1],
        [3, 1, 3],
        [6, 1, 0],
        [0, 1, 6],
        [7, 1, 0],
        [2, 1, 5],
        [7, 7, 0],
    ];
    for split in splits {
        let (read, after, value) = interleaved(0, split, End::Abort, MESSAGE);
        assert_eq!(
            (read, after),
            (mistyped("ByteString"), Ok(value)),
            "split {split:?}"
        );
    }
}

/// 3 chunks of a dropped read, 5 of another read, and 1 more of the dropped read, 9
/// in all, leave the other read its answer on each seed.
#[test]
fn an_interleave_of_two_answers_leaves_the_second_its_answer_on_each_seed() {
    for seed in 0..32 {
        let (read, after, value) = interleaved(seed, [3, 5, 1], End::Abort, MESSAGE);
        assert_eq!(
            (read, after),
            (mistyped("ByteString"), Ok(value)),
            "seed {seed}"
        );
    }
}

/// The largest message is a bound on each answer, so the 7 intermediate chunks of each
/// of two answers queue at once, with more bytes than the largest message, and each
/// answer arrives.
#[test]
fn the_largest_message_bounds_each_answer_not_the_queue() {
    let message = u32::try_from(10 * super::CHUNK).expect("fits");
    let (read, after, value) = interleaved(0, [7, 7, 0], End::Rest, message);
    assert_eq!((read, after), (mistyped("ByteString"), Ok(value)));
}

/// The body of each forged chunk of [`forged`] in a test of a length, in bytes.
const BODY: usize = 64_000;

/// An intermediate chunk of the request id `id`, which no request has, with the
/// headers of the MSG chunk `chunk` and a body of `body` bytes.
fn forged(chunk: &[u8], id: u32, body: usize) -> Vec<u8> {
    with_id(rebuilt(chunk, b'C', &vec![0; body]), id)
}

/// The answers of two reads of [`VALUE`], and the value of the server. Before the
/// answer of the first, the relay sends `counts[i]` forged intermediate chunks of the
/// request id `1_000_000 + i`, each with a body of `body` bytes. The client takes
/// messages of up to `message` bytes.
fn after_forged(
    counts: &[usize],
    body: usize,
    message: u32,
) -> (Result<Value, Fault>, Result<Value, Fault>, Value) {
    let counts = counts.to_vec();
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let armed = Cell::new(false);
            let order = |held: Vec<Vec<u8>>| {
                let mut out = Vec::new();
                for (offset, &count) in (0u32..).zip(&counts) {
                    let chunk = forged(&held[0], 1_000_000 + offset, body);
                    out.extend(std::iter::repeat_n(chunk, count));
                }
                out.extend(held);
                out
            };
            let body = async {
                let mut client = connect(&mut peer, &url, message).await;
                armed.set(true);
                let read = client.read_one(&VALUE).await;
                assert!(!armed.get(), "the relay sent the forged chunks");
                let after = client.read_one(&VALUE).await;
                client.close().await;
                (read, after)
            };
            let rewrite = reorder(&armed, 1, order);
            let reads = relayed(&node, &Relay::default(), rewrite, body).await;
            (reads.0, reads.1, peer.value)
        })
        .await
    })
    .0
}

/// The answers of a client whose channel closed at a chunk past a bound.
fn closed() -> (Result<Value, Fault>, Result<Value, Fault>) {
    (
        Err(Fault::status(Status::BAD_SECURE_CHANNEL_CLOSED)),
        Err(Fault::status(Status::BAD_SERVER_NOT_CONNECTED)),
    )
}

/// [`CHUNKS`] intermediate chunks of one request id queue, and one more closes the
/// channel, also with no final chunk.
#[test]
fn the_intermediate_chunks_of_one_request_id_stop_at_the_bound() {
    let chunks = usize::try_from(CHUNKS).expect("fits");
    let (read, after, value) = after_forged(&[chunks], 1, MESSAGE);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[chunks + 1], 1, MESSAGE);
    assert_eq!((read, after), closed());
}

/// Intermediate chunks of one request id whose bodies are the largest message in all
/// queue, and one more byte closes the channel, also with no final chunk.
#[test]
fn the_intermediate_chunks_of_one_request_id_stop_at_the_largest_message() {
    let message = u32::try_from(3 * BODY).expect("fits");
    let (read, after, value) = after_forged(&[3], BODY, message);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[2, 1, 3], BODY, message);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[4], BODY, message);
    assert_eq!((read, after), closed());
}

/// The queue of a channel holds at most the intermediate chunks of [`READS`] answers
/// of [`CHUNKS`] chunks, across all request ids, as a server can send chunks of ids
/// that no request has.
#[test]
fn the_queue_of_a_channel_stops_at_the_chunks_of_the_answers_in_flight() {
    let chunks = to_usize(CHUNKS);
    let queued = usize::from(READS) * (chunks - 1);
    let mut full = vec![chunks; queued / chunks];
    full.push(queued % chunks);
    let (read, after, value) = after_forged(&full, 1, MESSAGE);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let mut past = full;
    past.push(1);
    let (read, after, _) = after_forged(&past, 1, MESSAGE);
    assert_eq!((read, after), closed());
}

fn to_usize(count: u32) -> usize {
    usize::try_from(count).expect("fits")
}

/// The values of a read, or its status.
type Read = Result<Vec<Result<Value, Fault>>, Status>;

/// One read of `count` nodes of [`CHUNKS`] chunks each, on a server with a
/// `MaxNodesPerRead` of 1, so the client sends `count` requests at once. The relay
/// holds the `count` answers and sends them interleaved: chunk 0 of each, then chunk 1
/// of each, and so on. Gives the read, a read of [`VALUE`] after it, and the value.
fn interleaved_reads(count: u16) -> (Read, Result<Value, Fault>, Value) {
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.set_per_read(1);
            let nodes: Vec<_> = (0..u32::from(count)).map(numbered).collect();
            for node in &nodes {
                peer.server.add_bytes(node, 7 * super::CHUNK);
            }
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let armed = Cell::new(false);
            let (answers, chunks) = (usize::from(count), to_usize(CHUNKS));
            let order = |held: Vec<Vec<u8>>| {
                let kinds: Vec<_> = held.iter().map(|chunk| chunk[3]).collect();
                for answer in kinds.chunks(chunks) {
                    assert_eq!(*answer, *b"CCCCCCCF", "an answer of 8 chunks");
                }
                (0..chunks)
                    .flat_map(|index| (0..answers).map(move |a| a * chunks + index))
                    .map(|at| held[at].clone())
                    .collect()
            };
            let body = async {
                let mut client = connect(&mut peer, &url, MESSAGE).await;
                let nodes = fits(client.nodes(&nodes));
                assert_eq!(nodes.counts(), vec![1; answers]);
                armed.set(true);
                let read = client.read(&nodes).await.map(<[_]>::to_vec);
                assert!(!armed.get(), "the relay sent the held chunks");
                let after = client.read_one(&VALUE).await;
                client.close().await;
                (read, after)
            };
            let rewrite = reorder(&armed, chunks * answers, order);
            let reads = relayed(&node, &Relay::default(), rewrite, body).await;
            (reads.0, reads.1, peer.value)
        })
        .await
    })
    .0
}

/// The queue holds the chunks of each answer that the client has in flight, so
/// [`READS`] answers of [`CHUNKS`] chunks, all interleaved, each arrive, and the
/// channel stays open.
#[test]
fn the_answers_of_each_read_in_flight_interleaved_each_arrive() {
    let (read, after, value) = interleaved_reads(READS);
    let expected = vec![mistyped("ByteString"); usize::from(READS)];
    assert_eq!((read, after), (Ok(expected), Ok(value)));
}
