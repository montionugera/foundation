use std::cell::Cell;
use std::mem;

/// The bytes of the headers of a chunk with the security mode None: the message header
/// (8), the channel (4), the token (4), and the sequence header (8).
const HEADER: usize = 24;

/// The chunk of the kind `kind` (`b'C'`, `b'F'`, or `b'A'`) with the message,
/// security, and sequence headers of the MSG chunk `chunk`, then `body`.
pub(super) fn rebuilt(chunk: &[u8], kind: u8, body: &[u8]) -> Vec<u8> {
    // The policy None adds nothing past the headers.
    let mut rebuilt = chunk[..HEADER].to_vec();
    rebuilt[3] = kind;
    rebuilt.extend_from_slice(body);
    let size = u32::try_from(rebuilt.len()).expect("fits");
    rebuilt[4..8].copy_from_slice(&size.to_le_bytes());
    rebuilt
}

/// The body of the MSG chunk `chunk`.
pub(super) fn body(chunk: &[u8]) -> &[u8] {
    &chunk[HEADER..]
}

/// `chunk` with the request id `id`.
pub(super) fn with_id(mut chunk: Vec<u8>, id: u32) -> Vec<u8> {
    chunk[20..24].copy_from_slice(&id.to_le_bytes());
    chunk
}

/// The abort of `chunk` with the Error `code` and a null Reason (Part 6, 6.7.3).
pub(super) fn coded(chunk: &[u8], code: u32) -> Vec<u8> {
    let mut body = code.to_le_bytes().to_vec();
    body.extend_from_slice(&[0xFF; 4]);
    rebuilt(chunk, b'A', &body)
}

/// The abort of `chunk` with the Error `BadResponseTooLarge`.
pub(super) fn too_large(chunk: &[u8]) -> Vec<u8> {
    coded(chunk, 0x80B9_0000)
}

/// A rewrite of the relay that, once `armed`, holds the next `count` MSG chunks and
/// sends the chunks that `order` makes of them in their place, then disarms. Each MSG
/// chunk from the first held one on gets the next sequence number, as the client
/// checks them.
pub(super) fn reorder(
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
            let sequence = chunk[16..20].try_into().expect("4 bytes");
            next.get_or_insert(u32::from_le_bytes(sequence));
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
