//! A `send` of a block of at most 1452 bytes, which noq-proto copies, allocates
//! nothing in transport. A longer one allocates once: the shared count of the slice
//! that noq-proto keeps.
//!
//! noq-proto copies short writes into one segment of at most 1452 bytes, and a
//! message whose length prefix and body pass that flushes the segment, which
//! allocates. So the largest copied message here is 1450 bytes, and each size runs
//! alone, so that the segment's capacity is steady.

use transport::Class;

use crate::ALLOCATOR;
use crate::common::filled;
use crate::session::{PAUSE, ROUNDS, WARMUP, WIDE, poll, run};

/// The bytes of each message, and its allocations after warm-up.
const SENDS: [(usize, u64); 4] = [
    (1, 0),
    (64, 0),
    (1450, 0),
    // The shared count of the slice, and the flush of noq-proto's segment.
    (1453, 2),
];

pub(crate) fn main() {
    for (bytes, allocations) in SENDS {
        run(WIDE, async move |session, pool, clock| {
            let opened = session.open_sender(Class::Complete).await;
            let mut sender = opened.expect("a stream");
            for round in 0..WARMUP + ROUNDS {
                clock.sleep(PAUSE).await;
                let block = filled(&pool, bytes);
                let count = ALLOCATOR.count(|| poll(&mut sender, block, &[])).1;
                if round >= WARMUP {
                    assert_eq!(count, allocations, "a send of {bytes} bytes");
                }
            }
            sender.finish().expect("finished");
        });
    }
}
