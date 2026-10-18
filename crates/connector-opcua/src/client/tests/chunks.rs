use std::cell::Cell;
use std::mem;

use super::abort::{sequence, too_large};
use super::{FITS, RELAY, Relay, VALUE, drop_read, mistyped, relayed, run, serve};
use crate::connection::CHUNKS;
use crate::ffi::Status;
use crate::value::{Fault, Value};

/// A rewrite of the relay that, once `armed`, holds the next `count` MSG chunks and
/// sends the chunks that `order` makes of them in their place. Each MSG chunk from the
/// first held one on gets the next sequence number, as the client checks them.
fn reorder(
    armed: &Cell<bool>,
    count: usize,
    mut order: impl FnMut(Vec<Vec<u8>>) -> Vec<Vec<u8>>,
) -> impl FnMut(&mut Vec<u8>) {
    let mut held = Vec::new();
    let mut next = None::<u32>;
    move |chunk| {
        if !chunk.starts_with(b"MSG") {
            return;
        }
        if armed.get() {
            next.get_or_insert(u32::from_le_bytes(sequence(chunk)));
            held.push(mem::take(chunk));
            if held.len() < count {
                return;
            }
            armed.set(false);
            for mut sent in order(mem::take(&mut held)) {
                renumber(&mut next, &mut sent);
                chunk.extend_from_slice(&sent);
            }
        } else {
            renumber(&mut next, chunk);
        }
    }
}

/// Gives `chunk` the sequence number `next` and steps it, when it is set.
fn renumber(next: &mut Option<u32>, chunk: &mut [u8]) {
    if let Some(number) = next {
        chunk[16..20].copy_from_slice(&number.to_le_bytes());
        *number += 1;
    }
}

/// The answers of a dropped read D of [`FITS`] (8 chunks), then of a read R of it and
/// of a read of [`VALUE`], and the value of the server. The relay sends the first `d`
/// intermediate chunks of D, then the first `r` chunks of R, then `between` more
/// intermediate chunks of D, then the abort of D when `aborts`, else the rest of D,
/// and then the rest of R. The client takes messages of up to `message` bytes, or the
/// default of open62541 for `None`.
fn interleaved(
    seed: u64,
    [d, r, between]: [usize; 3],
    aborts: bool,
    message: Option<u32>,
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
                if aborts {
                    out.push(too_large(&dropped[7]));
                } else {
                    out.extend_from_slice(&dropped[d + between..]);
                }
                out
            };
            let reads = relayed(&node, &Relay::default(), reorder(&armed, 8 + r, order), {
                async {
                    let mut client = match message {
                        Some(size) => peer.limited(&url, None, Some(size)).await,
                        None => peer.connect(&url).await.expect("the client connects"),
                    };
                    assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
                    armed.set(true);
                    drop_read(&mut client, &FITS).await;
                    let read = client.read_one(&FITS).await;
                    assert!(!armed.get(), "the relay sent the held chunks");
                    let after = client.read_one(&VALUE).await;
                    client.close().await;
                    (read, after)
                }
            })
            .await;
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
    for aborts in [true, false] {
        let (read, after, value) = interleaved(0, [3, 6, 0], aborts, None);
        assert_eq!(
            (read, after),
            (mistyped("ByteString"), Ok(value)),
            "aborts {aborts}"
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
        let (read, after, value) = interleaved(0, split, true, None);
        assert_eq!(
            (read, after),
            (mistyped("ByteString"), Ok(value)),
            "split {split:?}"
        );
    }
}

#[test]
fn an_interleave_of_two_answers_leaves_the_second_its_answer_on_each_seed() {
    for seed in 0..32 {
        let (read, after, value) = interleaved(seed, [3, 2, 2], true, None);
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
    let (read, after, value) = interleaved(0, [7, 7, 0], false, Some(message));
    assert_eq!((read, after), (mistyped("ByteString"), Ok(value)));
}

/// The body of each forged chunk of [`forged`] in a test of a length, in bytes.
const BODY: usize = 64_000;

/// An intermediate chunk of the request id `id`, which no request has, with the
/// headers of the MSG chunk `chunk` and a body of `body` bytes.
fn forged(chunk: &[u8], id: u32, body: usize) -> Vec<u8> {
    // The policy None adds nothing past the 24 bytes of headers.
    let mut forged = chunk[..24].to_vec();
    forged[3] = b'C';
    forged[20..24].copy_from_slice(&id.to_le_bytes());
    forged.resize(24 + body, 0);
    let size = u32::try_from(forged.len()).expect("fits");
    forged[4..8].copy_from_slice(&size.to_le_bytes());
    forged
}

/// The answers of two reads of [`VALUE`], and the value of the server. Before the
/// answer of the first, the relay sends `counts[i]` forged intermediate chunks of the
/// request id `1_000_000 + i`, each with a body of `body` bytes. The client takes
/// messages of up to `message` bytes, or the default of open62541 for `None`.
fn after_forged(
    counts: &[usize],
    body: usize,
    message: Option<u32>,
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
            let reads = relayed(&node, &Relay::default(), reorder(&armed, 1, order), {
                async {
                    let mut client = match message {
                        Some(size) => peer.limited(&url, None, Some(size)).await,
                        None => peer.connect(&url).await.expect("the client connects"),
                    };
                    assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
                    armed.set(true);
                    let read = client.read_one(&VALUE).await;
                    assert!(!armed.get(), "the relay sent the forged chunks");
                    let after = client.read_one(&VALUE).await;
                    client.close().await;
                    (read, after)
                }
            })
            .await;
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
    let (read, after, value) = after_forged(&[chunks], 1, None);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[chunks + 1], 1, None);
    assert_eq!((read, after), closed());
}

/// Intermediate chunks of one request id whose bodies are the largest message in all
/// queue, and one more byte closes the channel, also with no final chunk.
#[test]
fn the_intermediate_chunks_of_one_request_id_stop_at_the_largest_message() {
    let message = Some(u32::try_from(3 * BODY).expect("fits"));
    let (read, after, value) = after_forged(&[3], BODY, message);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[2, 1, 3], BODY, message);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let (read, after, _) = after_forged(&[4], BODY, message);
    assert_eq!((read, after), closed());
}

/// The queue of a channel holds at most [`CHUNKS`] squared intermediate chunks of all
/// request ids, as a server can send chunks of ids that no request has.
#[test]
fn the_queue_of_a_channel_stops_at_the_square_of_the_bound() {
    let chunks = usize::try_from(CHUNKS).expect("fits");
    let full = vec![chunks; chunks];
    let (read, after, value) = after_forged(&full, 1, None);
    assert_eq!((read, after), (Ok(value), Ok(value)));
    let mut past = full;
    past.push(1);
    let (read, after, _) = after_forged(&past, 1, None);
    assert_eq!((read, after), closed());
}
