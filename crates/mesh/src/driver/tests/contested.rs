//! Tests of the check of the leader that no two joins give one node key two public
//! keys, and of a join that repeats one that applied.

use super::*;

/// The join of node `id` whose card holds the public key of node 5, which signed it.
fn twin(id: u8) -> Change {
    let card = card::Card {
        public_key: public(5),
        ..common::member(id).card.card().clone()
    };
    let signed = card::Signed::sign(key(id), card, &common::private(5));
    join_change_with(&signed, 7, Stamp::EPOCH)
}

fn contested(id: u8) -> Error {
    Error::Contested { key: key(id) }
}

// Node 2 grants the election and answers no append, so no entry commits.
#[test]
fn a_leader_refuses_a_join_whose_key_has_another_public_key_in_its_log() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        let first = elect(&mesh).await;
        assert_eq!(mesh.propose(join_change(4)).await, Ok(after(first, 1)));
        let refused = mesh.propose(twin(4)).await;
        assert_eq!(refused, Err(contested(4)));
        let text = format!(
            "node key {} has another public key in a member or in a join that is not \
             applied yet",
            key(4)
        );
        assert_eq!(refused.unwrap_err().to_string(), text);
        assert_eq!(mesh.propose(join_change(4)).await, Ok(after(first, 2)));
        assert_eq!(mesh.answer(public(2), twin(4)).await, Err(contested(4)));
        assert_eq!(mesh.propose(join_change(3)).await, Ok(after(first, 3)));
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(first, 4)));
    });
}

#[test]
fn a_leader_refuses_a_join_whose_key_has_another_public_key_in_a_member() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[1]).await.unwrap();
        let first = lead(&mesh, &node.clock(), home_to(1)).await;
        assert_eq!(mesh.propose(twin(2)).await, Err(contested(2)));
        assert_eq!(mesh.answer(public(1), twin(2)).await, Err(contested(2)));
        assert_eq!(mesh.propose(join_change(2)).await, Ok(after(first, 1)));
        assert_eq!(mesh.propose(twin(3)).await, Ok(after(first, 2)));
    });
}

// Only the leader checks, so a follower whose log holds the join of node 4 proposes
// the twin, and `raft` refuses it.
#[test]
fn a_follower_gives_not_leader_for_a_join_that_its_log_contests() {
    use super::unapplied::{changes, write};
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &[2, 3]).await.unwrap();
        write(&mesh, changes(&[join_change(4)])).await;
        let leader = Some(key(2));
        let not_leader = Error::Raft(raft::Error::NotLeader { leader });
        assert_eq!(mesh.propose(twin(4)).await, Err(not_leader));
    });
}

// The leader refuses the forwarded twin of a voter with code 19, so the voter does
// not forward it again, and the twin takes no entry.
#[test]
fn a_voter_gives_contested_for_a_join_that_the_leader_refuses() {
    for run in 0..4 {
        let (mut cluster, leader, follower, at) = Cluster::led(run);
        cluster
            .board
            .lock()
            .unwrap()
            .joins
            .insert(follower, twin(follower));
        cluster.run(seconds(5));
        let board = cluster.board();
        let settled = vec![(follower, Err(contested(follower)))];
        assert_eq!(board.settled, settled, "run {run}");
        assert_eq!(
            board.contested,
            [(leader, contested(follower))],
            "run {run}"
        );
        let script = [(leader, [encoded(&home_to(9))].into())].into();
        cluster.board.lock().unwrap().script = script;
        cluster.run(seconds(5));
        assert_eq!(cluster.board().at, [after(at, 1)], "run {run}");
    }
}

// MESH DRIVER: a repeat of a `Join` finds its node a member and is refused as
// `Unfit::Duplicate` before the ticket counts a use. Ticket 8 admits one use.
#[test]
fn a_repeat_of_a_join_with_a_one_use_ticket_is_refused_as_duplicate() {
    solo(|node, tasks| async move {
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", false, LAST)]).await;
        let join = join_change_with(&common::member(4).card, 8, NOW);
        let first = mesh.attempt().unwrap().settle(join.clone()).await;
        assert_eq!(first, Ok(Some(Ok(()))));
        let repeat = mesh.attempt().unwrap().settle(join).await;
        let duplicate = Refused::from(Unfit::Duplicate { key: key(4) });
        assert_eq!(repeat, Ok(Some(Err(duplicate))));
    });
}
