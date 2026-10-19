//! Tests of the answer of a voter to a join request.

use super::*;
use crate::refusal::Refusal;

// The single-use ticket refuses a second stamp, so the second answer comes from
// the applied member.
#[test]
fn a_voter_admits_a_join_and_admits_it_again_after_it_applied() {
    solo(|node, tasks| async move {
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", false, LAST)]).await;
        let time = synced(&node);
        let admitted = mesh.admit(public(4), &time, request(4, 8, &["disk"])).await;
        assert_eq!(admitted, Ok(Ok(())));
        let member = mesh.member(key(4)).unwrap();
        assert_eq!(member.card, common::member(4).card);
        let again = mesh.admit(public(4), &time, request(4, 8, &["disk"])).await;
        assert_eq!(again, Ok(Ok(())));
        assert_eq!(mesh.member(key(4)), Some(member));
        let used = ticket::Refused::Used {
            public_key: public(8),
        };
        let other = mesh.admit(public(5), &time, request(5, 8, &[])).await;
        let refused = Refusal::Refused(region::Refused::Ticket(used));
        assert_eq!(other, Ok(Err(refused)));
    });
}

// Only the holder of the card's private key proves its public key, so no other
// peer asks for a join in its name.
#[test]
fn a_join_from_a_peer_that_is_not_the_card_key_is_spoofed() {
    solo(|node, tasks| async move {
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", true, LAST)]).await;
        let time = synced(&node);
        let spoofed = mesh.admit(public(5), &time, request(4, 8, &[])).await;
        assert_eq!(spoofed, Err(Error::Spoofed { from: key(4) }));
        assert_eq!(mesh.member(key(4)), None);
    });
}

#[test]
fn a_join_that_the_voter_cannot_admit_gives_the_refusal() {
    solo(|node, tasks| async move {
        let unsynced = clock::Clock::new(node.clock()).1;
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", true, LAST)]).await;
        let time = synced(&node);
        let unknown = region::Refused::Unknown {
            public_key: public(9),
        };
        let cases = [
            (&time, request(4, 9, &[]), Refusal::Refused(unknown)),
            (&unsynced, request(4, 8, &[]), Refusal::Unsynced),
        ];
        for (time, request, refusal) in cases {
            let refused = mesh.admit(public(4), time, request).await;
            assert_eq!(refused, Ok(Err(refusal)));
        }
        assert_eq!(mesh.member(key(4)), None);
    });
}

// A refused join of the log binds key 4 to the public key of its card, so the
// leader refuses a join of key 4 under another key.
#[test]
fn a_join_of_a_bound_key_under_another_public_key_is_contested() {
    solo(|node, tasks| async move {
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", true, LAST)]).await;
        let time = synced(&node);
        let unknown = join_change_with(&common::member(4).card, 9, Stamp::EPOCH);
        mesh.propose(unknown).await.unwrap();
        let mut card = common::member(4).card.card().clone();
        card.public_key = public(5);
        let card = card::Signed::sign(key(4), card, &common::private(5));
        let request = Request {
            ticket: public(8),
            card: card.clone().into(),
            admission: common::ticket(8).admission(&card),
            status: BTreeSet::new(),
        };
        let contested = mesh.admit(public(5), &time, request).await;
        assert_eq!(contested, Ok(Err(Refusal::Contested)));
        assert_eq!(mesh.member(key(4)), None);
    });
}

#[test]
fn a_node_that_is_not_a_voter_or_that_stopped_admits_no_join() {
    solo(|node, tasks| async move {
        let time = synced(&node);
        let mesh = open(&node, &tasks, 2, &IDS, &[1]).await.unwrap();
        let refused = mesh.admit(public(4), &time, request(4, 8, &[])).await;
        assert_eq!(refused, Ok(Err(Refusal::NoVote)));
    });
    solo(|node, tasks| async move {
        let time = synced(&node);
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        stop_by_an_empty_change(&mesh).await;
        let refused = mesh.admit(public(4), &time, request(4, 8, &[])).await;
        assert_eq!(refused, Ok(Err(Refusal::Stopped)));
    });
}
