use super::*;

/// Writes a line of `len` bytes, of one sample.
fn line(len: usize) -> impl FnOnce(&mut Vec<u8>) -> Counts {
    move |bytes| {
        bytes.resize(bytes.len() + len, b'x');
        Counts {
            samples: 1,
            ..Counts::default()
        }
    }
}

/// A line over 4 MiB goes alone in a body, with no empty body before it. A body of
/// exactly 4 MiB keeps its last line, and a line that takes a body of other lines past
/// 4 MiB moves to the next body.
#[test]
fn writes_a_line_over_4_mib_alone_and_moves_a_line_past_4_mib() {
    let mut bodies = Bodies::default();
    for len in [BYTES_MAX + 1, 10, BYTES_MAX - 10, 1, BYTES_MAX] {
        bodies.line(line(len));
    }
    let closed: Vec<_> = (bodies.closed.iter())
        .map(|taken| (taken.body.len(), taken.ack.is_some()))
        .collect();
    assert_eq!(
        closed,
        [(BYTES_MAX + 1, false), (BYTES_MAX, false), (1, false)]
    );
    assert_eq!((bodies.bytes.len(), bodies.lines), (BYTES_MAX, 1));
    assert_eq!(bodies.pending.samples, 5);
}
