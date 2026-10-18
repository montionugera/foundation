//! Tests of the change of the card of a member: the bound of one proposal of each
//! member each minute, and `Mesh::set_addresses`.

use card::addresses::Addresses;

use super::home::{Leader, run};
use super::*;

/// A `Card` change of the card of node `id` at `version`, which its own key signed.
fn card_change(id: u8, version: u64) -> Change {
    card_at(
        id,
        common::member(id).card.card().addresses.clone(),
        version,
    )
}

/// As [`card_change`], with `addresses`.
fn card_at(id: u8, addresses: Addresses, version: u64) -> Change {
    let card = card::Card {
        addresses,
        version,
        ..common::member(id).card.card().clone()
    };
    let signed = card::Signed::sign(key(id), card, &private(id));
    Change::Card(Box::new(unchecked(&signed)))
}

/// The address of node `id` on its port, and a second one at `port`.
fn moved(id: u8, port: u16) -> Addresses {
    let second = SocketAddr::new(Ipv4Addr::new(10, 0, 0, id).into(), port);
    Addresses::new(vec![Address::Udp(address(id)), Address::Udp(second)]).unwrap()
}

/// The card of node `id` on `mesh`.
fn card_of(mesh: &Mesh, id: u8) -> card::Card {
    mesh.member(key(id)).unwrap().card.card().clone()
}

/// The card in `change`.
fn card_in(change: &Change) -> &card::Card {
    let Change::Card(card) = change else {
        panic!("{change:?} is not a card change");
    };
    &card.card
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

#[test]
fn a_card_change_holds_back_another_for_one_minute() {
    let at = Monotonic(5);
    assert!(recent(at, at));
    assert!(recent(at, Monotonic(60_000_000_004)));
    assert!(!recent(at, Monotonic(60_000_000_005)));
    assert!(recent(Monotonic(u64::MAX), Monotonic(u64::MAX)));
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

#[test]
fn set_addresses_proposes_the_card_with_them_at_the_next_version() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        assert_eq!(mesh.set_addresses(moved(1, 9)).await, Ok(()));
        let card = card::Card {
            addresses: moved(1, 9),
            version: 2,
            ..common::member(1).card.card().clone()
        };
        assert_eq!(card_of(&mesh, 1), card);
        // A repeat proposes nothing, so the bound does not refuse it.
        assert_eq!(mesh.set_addresses(moved(1, 9)).await, Ok(()));
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(at, 2)));
    });
}

// The call that gets `Soon` signed version 3 before its proposal.
#[test]
fn a_second_call_within_a_minute_gives_soon_and_one_after_it_applies() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
        let clock = node.clock();
        lead(&mesh, &clock, home_to(1)).await;
        assert_eq!(mesh.set_addresses(moved(1, 9)).await, Ok(()));
        let done = clock.now();
        assert_eq!(mesh.set_addresses(moved(1, 10)).await, Err(Error::Soon));
        assert_eq!(card_of(&mesh, 1).addresses, moved(1, 9));
        clock.sleep_until(done + Span::MINUTE).await;
        assert_eq!(mesh.set_addresses(moved(1, 10)).await, Ok(()));
        let card = card_of(&mesh, 1);
        assert_eq!((card.addresses, card.version), (moved(1, 10), 4));
    });
}

#[test]
fn a_member_that_is_not_a_voter_sets_no_addresses() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[2]).await.unwrap();
        let set = now(pin!(mesh.set_addresses(moved(1, 9)))).await;
        assert_eq!(set, Poll::Ready(Err(Error::NoVote)));
    });
}

// The write of the file finds no block, so the call signs nothing until a block is
// free.
#[test]
fn set_addresses_signs_again_while_the_pool_has_no_block() {
    solo(|node, tasks| async move {
        let pool = small_pool();
        let config = Config {
            pool: Rc::clone(&pool),
            ..config(&node, &tasks, 1, &[1], &[1]).await
        };
        let mesh = Mesh::start(config).await.unwrap();
        let clock = node.clock();
        lead(&mesh, &clock, home_to(1)).await;
        let held = fill(&pool);
        let mut set = pin!(mesh.set_addresses(moved(1, 9)));
        for _ in 0..3 {
            assert!(now(set.as_mut()).await.is_pending());
            clock.sleep(TICK).await;
        }
        drop(held);
        assert_eq!(set.await, Ok(()));
        assert_eq!(card_of(&mesh, 1).version, 2);
    });
}

// Node 2 leads, and commits a card of node 1 at version 3 just before the card of
// the call. The apply refuses the card of the call as behind, so the call signs the
// next version above 3.
#[test]
fn a_card_of_a_later_version_that_applied_first_takes_the_next_version() {
    let call = |_, mesh: Mesh| async move {
        let set = mesh.set_addresses(moved(1, 9)).await;
        (set, card_of(&mesh, 1))
    };
    let leader = |mut leader: Leader| async move {
        let (first, mut asked) = leader.proposal().await;
        leader.answer(&mut asked, at(2)).await.unwrap();
        let later = (encoded(&card_at(1, moved(1, 10), 3)), at(1));
        let each = vec![later, (encoded(&first), at(2))];
        leader.append_each(Position::default(), each).await;
        let (second, mut asked) = leader.proposal().await;
        leader.answer(&mut asked, at(3)).await.unwrap();
        let each = vec![(encoded(&second), at(3))];
        leader.append_each(at(2), each).await;
        leader.rest(seconds(2)).await;
        [first, second].map(|change| card_in(&change).clone())
    };
    let ((set, card), [first, second]) = run(call, leader);
    assert_eq!(set, Ok(()));
    assert_eq!((first.addresses.clone(), first.version), (moved(1, 9), 2));
    assert_eq!(
        second,
        card::Card {
            version: 4,
            ..first
        }
    );
    assert_eq!(card, second);
}

// The leader counts the card that the follower forwards, and refuses the next one
// within the minute with code 18.
#[test]
fn a_follower_sets_its_addresses_through_the_leader_once_a_minute() {
    let (mut cluster, _, follower, _) = Cluster::led(0);
    cluster
        .board
        .lock()
        .unwrap()
        .addresses
        .insert(follower, moved(follower, 9));
    cluster.run(seconds(5));
    cluster
        .board
        .lock()
        .unwrap()
        .addresses
        .insert(follower, moved(follower, 10));
    cluster.run(seconds(5));
    let board = cluster.board();
    let card = |addresses, version| card::Card {
        addresses,
        version,
        ..create_voter(follower).card.card().clone()
    };
    let moves = [
        (follower, Ok(()), card(moved(follower, 9), 2)),
        (follower, Err(Error::Soon), card(moved(follower, 9), 2)),
    ];
    assert_eq!(board.moved, moves);
    cluster.run(seconds(60));
    cluster
        .board
        .lock()
        .unwrap()
        .addresses
        .insert(follower, moved(follower, 10));
    cluster.run(seconds(5));
    let board = cluster.board();
    let card = card(moved(follower, 10), 4);
    assert_eq!(board.moved, [(follower, Ok(()), card)]);
}

/// Two calls at once write the file of the last signed version in turn, so neither
/// finds it busy. The second card goes over the bound.
#[test]
fn two_calls_at_once_write_the_file_in_turn() {
    for seed in 0..32 {
        solo_at(seed, move |node, tasks| async move {
            let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
            lead(&mesh, &node.clock(), home_to(1)).await;
            let mut calls = [
                Box::pin(mesh.set_addresses(moved(1, 9))),
                Box::pin(mesh.set_addresses(moved(1, 10))),
            ];
            let mut done: [Option<Result<(), Error>>; 2] = [None, None];
            poll_fn(|cx| {
                for (call, out) in calls.iter_mut().zip(done.iter_mut()) {
                    if out.is_none()
                        && let Poll::Ready(result) = call.as_mut().poll(cx)
                    {
                        *out = Some(result);
                    }
                }
                if done.iter().all(Option::is_some) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            let mut done = done.map(Option::unwrap);
            done.sort_by_key(Result::is_err);
            assert_eq!(done, [Ok(()), Err(Error::Soon)], "seed {seed}");
        });
    }
}
