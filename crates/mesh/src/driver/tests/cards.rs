//! Tests of the change of the card of a member: the bound of one proposal of each
//! member each minute, and `Mesh::set_addresses`.

use super::*;

/// A `Card` change of the card of node `id` at `version`, which its own key signed.
fn card_change(id: u8, version: u64) -> Change {
    let card = card::Card {
        version,
        ..common::member(id).card.card().clone()
    };
    let signed = card::Signed::sign(key(id), card, &private(id));
    Change::Card(Box::new(unchecked(&signed)))
}

#[test]
fn a_leader_proposes_one_card_of_each_member_each_minute() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        let asked = node.clock().now();
        assert_eq!(mesh.propose(card_change(2, 2)).await, Ok(after(at, 1)));
        assert_eq!(mesh.propose(card_change(1, 2)).await, Ok(after(at, 2)));
        let minute = asked + Span::MINUTE;
        node.clock().sleep_until(minute - Span::from_nanos(1)).await;
        let soon = mesh.propose(card_change(2, 3)).await;
        assert_eq!(soon, Err(Error::Soon));
        let text = "a node proposed a card of this node less than a minute ago; call \
                    again after a minute";
        assert_eq!(soon.unwrap_err().to_string(), text);
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(at, 3)));
        node.clock().sleep_until(minute).await;
        assert_eq!(mesh.propose(card_change(2, 3)).await, Ok(after(at, 4)));
    });
}

/// A proposal that the group does not take does not count.
#[test]
fn a_card_that_a_follower_does_not_propose_does_not_count() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[2]).await.unwrap();
        for _ in 0..2 {
            let refused = mesh.propose(card_change(2, 2)).await;
            let follower = Error::Raft(raft::Error::NotLeader { leader: None });
            assert_eq!(refused, Err(follower));
        }
    });
}
