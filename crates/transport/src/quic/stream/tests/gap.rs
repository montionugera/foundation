//! The send window after a reset of a stream whose first unacknowledged frame is
//! lost.

use super::*;

#[test]
fn a_reset_after_a_loss_gives_back_each_byte_of_the_window() {
    testing::run(1, |shard| {
        let mut pair = connected(shard);
        let mut sender = open_sender(&mut pair, Class::Complete);
        let mut incoming = None;
        // Grows the congestion window until it takes a whole send window at once.
        for _ in 0..16 {
            fill(&mut pair, shard, &mut sender);
            pair.run(RUN);
            let incoming = incoming.get_or_insert_with(|| accept(&mut pair.server));
            let now = pair.now();
            drain(&mut pair.server, now, &mut incoming.receiver);
            pair.run(RUN);
        }
        pair.client.drops = 1;
        let full = fill(&mut pair, shard, &mut sender);
        // The acknowledgments of the later batches come back.
        pair.run(2 * DELAY + STEP);
        let now = pair.now();
        pair.client.endpoint.reset(now, &mut sender, Code(9));
        pair.run(RUN);
        let mut next = open_sender(&mut pair, Class::Complete);
        assert_eq!(fill(&mut pair, shard, &mut next), full);
    });
}
