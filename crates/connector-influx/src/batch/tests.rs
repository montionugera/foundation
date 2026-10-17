use super::*;

/// Writes a line of `len` bytes, of one sample.
fn line(len: usize) -> impl FnOnce(&mut Vec<u8>) -> Counts {
    move |bytes| {
        bytes.extend(iter::repeat_n(b'x', len));
        Counts {
            samples: 1,
            ..Counts::default()
        }
    }
}

/// A line over 4 MiB goes alone in a body, with no empty body before it. A body of
/// exactly 4 MiB keeps its last line, and a line that takes a body of other lines past
/// 4 MiB moves to the next body. Only this test reaches a line over 4 MiB: the records
/// of the test hub hold 64 KiB, and a channel name at most 255 bytes.
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
    assert_eq!(bodies.counts.samples, 5);
}

/// A body that grew to take the line that moves out of it keeps no room past its lines.
/// The next body has room for a moved line over 4 MiB and no more, and room for 4 MiB
/// and a moved line of at most 4 MiB. A body closed before a line opens the next with
/// room for the most bytes of a body.
#[test]
fn moves_a_line_past_4_mib_with_no_room_left_from_the_move() {
    let mut bodies = Bodies::default();
    for len in [BYTES_MAX - 10, 1, 6 << 20, 10, BYTES_MAX, 20] {
        bodies.line(line(len));
    }
    let closed: Vec<_> = (bodies.closed.iter())
        .map(|taken| (taken.body.len(), taken.body.capacity()))
        .collect();
    assert_eq!(
        closed,
        [
            (BYTES_MAX - 9, BYTES_MAX - 9),
            (6 << 20, 6 << 20),
            (10, 6 << 20),
            (BYTES_MAX, 2 * BYTES_MAX),
        ]
    );
    assert_eq!((bodies.bytes.len(), bodies.bytes.capacity()), (20, 6 << 20));
}
