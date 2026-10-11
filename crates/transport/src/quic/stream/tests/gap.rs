//! The send window and the held blocks after a reset of a stream with bytes that the
//! peer did not acknowledge.

use super::*;
use crate::quic::pair::Foreign;

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

#[test]
fn a_reset_frees_its_blocks_before_the_peer_acknowledges_it() {
    testing::run(1, |shard| {
        // A peer that gives credit for 8 windows, so only the send window limits.
        let credit = VarInt::from_u32(8 << 20);
        let foreign = Foreign::new(shard, |config| {
            config.receive_window(credit).stream_receive_window(credit);
        });
        let mut pair = Pair::new(shard, Span::SECOND, DELAY);
        pair.foreign = Some(foreign);
        let (now, peer) = (pair.now(), pair::FOREIGN_KEY.public());
        pair.client.key = Some(pair.client.endpoint.connect(now, peer, pair::FOREIGN));
        pair.run(RUN);
        let own = Hello {
            window_bytes: 1 << 20,
            message_bytes_max: MESSAGE_MAX,
        };
        let connection = pair.foreign.as_mut().expect("a foreign peer").connection();
        raw(connection, Dir::Uni, &own.encode(), true);
        pair.run(RUN);
        // No packet of the peer arrives after this.
        pair.foreign = None;
        let before = shard.committed();
        let mut first = open_sender(&mut pair, Class::Complete);
        let full = fill(&mut pair, shard, &mut first);
        pair.run(RUN);
        let now = pair.now();
        pair.client.endpoint.reset(now, &mut first, Code(9));
        pair.run(RUN);
        let (now, mut second) = (pair.now(), open_sender(&mut pair, Class::Complete));
        for count in 1..full {
            let message = shard.block(&vec![count; MESSAGE_MAX]);
            write(&mut pair.client, now, &mut second, &[message]);
            let held = (shard.committed() - before) / MESSAGE_MAX;
            assert!(held <= (1 << 20) / MESSAGE_MAX + 2, "{held} blocks");
        }
    });
}
