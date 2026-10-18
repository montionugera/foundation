use std::cell::{Cell, RefCell};
use std::mem;

use super::{
    CHUNK, FITS, RELAY, Relay, VALUE, Values, drop_reads, mistyped, relayed, run, serve,
};
use crate::ffi::Status;
use crate::node::Node;
use crate::value::{Fault, Value};

/// The abort chunk of the final MSG chunk `chunk`: its message, security, and sequence
/// headers, then `body` (Part 6, 6.7.3).
fn of(chunk: &[u8], body: &[u8]) -> Vec<u8> {
    // The policy None adds nothing past the 24 bytes of headers.
    let mut abort = chunk[..24].to_vec();
    abort[3] = b'A';
    abort.extend_from_slice(body);
    let size = u32::try_from(abort.len()).expect("fits");
    abort[4..8].copy_from_slice(&size.to_le_bytes());
    abort
}

/// An abort of `chunk` with the Error `code` and a null Reason.
fn coded(chunk: &[u8], code: u32) -> Vec<u8> {
    let mut body = code.to_le_bytes().to_vec();
    body.extend_from_slice(&[0xFF; 4]);
    of(chunk, &body)
}

fn too_large(chunk: &[u8]) -> Vec<u8> {
    coded(chunk, 0x80B9_0000)
}

fn empty(chunk: &[u8]) -> Vec<u8> {
    of(chunk, &[])
}

/// An abort of `chunk` with the request id `u32::MAX`, which no request has.
fn unknown(chunk: &[u8]) -> Vec<u8> {
    let mut abort = too_large(chunk);
    abort[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
    abort
}

/// A rewrite of the relay that sends `abort` of the next `left` final chunks in their
/// place.
fn aborting(left: &Cell<u16>, abort: fn(&[u8]) -> Vec<u8>) -> impl FnMut(&mut Vec<u8>) {
    move |chunk| {
        if chunk.starts_with(b"MSGF") && left.get() > 0 {
            *chunk = abort(chunk);
            left.set(left.get() - 1);
        }
    }
}

/// After `dropped` dropped reads, the answers of `aborted` reads of `target` and of two
/// more reads of it, and the value of the server. The relay sends `abort` of the
/// answers of the dropped reads and of the `aborted` reads in their place.
fn reads(
    target: &'static Node,
    dropped: u16,
    aborted: u16,
    abort: fn(&[u8]) -> Vec<u8>,
) -> (Values, [Result<Value, Fault>; 2], Value) {
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.add_bytes(&FITS, 7 * CHUNK);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let left = Cell::new(0u16);
            let reads =
                relayed(&node, &Relay::default(), aborting(&left, abort), async {
                    let mut client =
                        peer.connect(&url).await.expect("the client connects");
                    // open62541 reads the namespace array after the connect.
                    assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
                    left.set(dropped + aborted);
                    drop_reads(&mut client, dropped).await;
                    let mut answers = Vec::new();
                    for _ in 0..aborted {
                        answers.push(client.read_one(target).await);
                    }
                    let after = client.read_one(target).await;
                    assert_eq!(left.get(), 0, "the server answered each read");
                    let again = client.read_one(target).await;
                    client.close().await;
                    (answers, [after, again])
                })
                .await;
            (reads.0, reads.1, peer.value)
        })
        .await
    })
    .0
}

/// An abort chunk answers its read with its Error, so after 16 of them the client
/// holds no read and a later read that the server answers in full gives the value.
#[test]
fn a_read_after_16_aborted_answers_gives_the_value() {
    let (aborted, [after, again], value) = reads(&VALUE, 0, 16, too_large);
    assert_eq!(
        aborted,
        vec![Err(Fault::status(Status::BAD_RESPONSE_TOO_LARGE)); 16]
    );
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An abort chunk with no Error ends its read with `BadDecodingError`.
#[test]
fn an_abort_with_no_error_ends_its_read_with_bad_decoding_error() {
    let (aborted, [after, again], value) = reads(&VALUE, 0, 1, empty);
    assert_eq!(
        aborted,
        vec![Err(Fault::status(Status::BAD_DECODING_ERROR))]
    );
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An abort chunk with a code that is not Bad holds no Error either, so it also ends
/// its read with `BadDecodingError`.
#[test]
fn an_abort_with_a_good_code_ends_its_read_with_bad_decoding_error() {
    let (aborted, [after, again], value) = reads(&VALUE, 0, 1, |chunk| coded(chunk, 0));
    assert_eq!(
        aborted,
        vec![Err(Fault::status(Status::BAD_DECODING_ERROR))]
    );
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An Uncertain code is not Bad, so an abort chunk with one ends its read with
/// `BadDecodingError`.
#[test]
fn an_abort_with_an_uncertain_code_ends_its_read_with_bad_decoding_error() {
    let (aborted, [after, again], value) =
        reads(&VALUE, 0, 1, |chunk| coded(chunk, 0x4000_0000));
    assert_eq!(
        aborted,
        vec![Err(Fault::status(Status::BAD_DECODING_ERROR))]
    );
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An abort ends the request of its id, not the newest request, so the abort of the
/// answer of a dropped read leaves the read after it in flight.
#[test]
fn the_abort_of_a_dropped_read_leaves_the_next_read_its_value() {
    let (aborted, [after, again], value) = reads(&VALUE, 1, 0, too_large);
    assert_eq!(aborted, Vec::new());
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An abort with an id that no request has ends no read, so its read times out, and
/// the client reads on.
#[test]
fn an_abort_with_an_unknown_id_ends_no_read() {
    let (aborted, [after, again], value) = reads(&VALUE, 0, 1, unknown);
    assert_eq!(aborted, vec![Err(Fault::status(Status::BAD_TIMEOUT))]);
    assert_eq!((after, again), (Ok(value), Ok(value)));
}

/// An abort after the 7 intermediate chunks of an answer of [`FITS`] deletes them, so
/// its read gives the Error of the abort, and frees their place in the count of
/// chunks, so each next answer of 8 chunks arrives.
#[test]
fn an_abort_after_intermediate_chunks_ends_its_read_and_frees_their_place() {
    let (aborted, [after, again], _) = reads(&FITS, 0, 1, too_large);
    assert_eq!(
        aborted,
        vec![Err(Fault::status(Status::BAD_RESPONSE_TOO_LARGE))]
    );
    assert_eq!(
        (after, again),
        (mistyped("ByteString"), mistyped("ByteString"))
    );
}

fn sequence(chunk: &[u8]) -> [u8; 4] {
    chunk[16..20].try_into().expect("4 bytes")
}

/// A dropped read D, then a read R of [`FITS`] (8 chunks). The relay holds the answer
/// of D and sends its abort after the first chunk of R when `inside`, else before it.
/// Gives the read of R, a read of [`VALUE`] after it, and the value.
fn interleaved(inside: bool) -> (Result<Value, Fault>, Result<Value, Fault>, Value) {
    run(0, async move |node| {
        serve(&node, 4, async |mut peer| {
            peer.server.add_bytes(&FITS, 7 * CHUNK);
            let url = format!("opc.tcp://{}:{RELAY}", node.addresses()[0]);
            let armed = Cell::new(false);
            let held = RefCell::new(None::<Vec<u8>>);
            let done = Cell::new(false);
            let rewrite = |chunk: &mut Vec<u8>| {
                if !armed.get() || done.get() || !chunk.starts_with(b"MSG") {
                    return;
                }
                let Some(dropped) = held.borrow_mut().take() else {
                    assert_eq!(chunk[3], b'F', "the answer of D is one chunk");
                    *held.borrow_mut() = Some(mem::take(chunk));
                    return;
                };
                assert_eq!(chunk[3], b'C', "the first chunk of R");
                let mut first = mem::take(chunk);
                let mut abort = too_large(&dropped);
                // The relay sends the two chunks in a new order, so each gets the
                // sequence number of the place it takes.
                let (early, late) = (sequence(&dropped), sequence(&first));
                let (a, b) = if inside {
                    (&mut first, &mut abort)
                } else {
                    (&mut abort, &mut first)
                };
                a[16..20].copy_from_slice(&early);
                b[16..20].copy_from_slice(&late);
                chunk.extend_from_slice(a);
                chunk.extend_from_slice(b);
                done.set(true);
            };
            let reads = relayed(&node, &Relay::default(), rewrite, async {
                let mut client = peer.connect(&url).await.expect("the client connects");
                assert_eq!(client.read_one(&VALUE).await, Ok(peer.value));
                armed.set(true);
                drop_reads(&mut client, 1).await;
                let read = client.read_one(&FITS).await;
                assert!(done.get(), "the relay sent the abort");
                let after = client.read_one(&VALUE).await;
                client.close().await;
                (read, after)
            })
            .await;
            (reads.0, reads.1, peer.value)
        })
        .await
    })
    .0
}

/// The abort of a dropped read before the chunks of another read leaves that read its
/// answer.
#[test]
fn an_abort_before_the_chunks_of_another_read_leaves_its_answer() {
    let (read, after, value) = interleaved(false);
    assert_eq!((read, after), (mistyped("ByteString"), Ok(value)));
}

/// An abort deletes only the chunks of its own request, so the abort of a dropped read
/// between the chunks of another read leaves that read its answer.
#[test]
fn an_abort_between_the_chunks_of_another_read_leaves_its_answer() {
    let (read, after, value) = interleaved(true);
    assert_eq!((read, after), (mistyped("ByteString"), Ok(value)));
}
