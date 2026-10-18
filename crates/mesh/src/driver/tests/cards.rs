//! Tests of the change of the card of a member: the bound of one proposal of each
//! member each minute, and `Mesh::set_addresses`.

use std::future::pending;
use std::net::Ipv6Addr;

use card::addresses::Addresses;

use super::home::{Leader, run, run_after};
use super::*;
use crate::driver::version::next;

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
        let text = "a card of the same member was proposed less than a minute ago; \
                    call again after a minute";
        assert_eq!(soon.unwrap_err().to_string(), text);
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(at, 3)));
        node.clock().sleep_until(minute).await;
        assert_eq!(mesh.propose(card_change(2, 3)).await, Ok(after(at, 4)));
    });
}

#[test]
fn a_repeat_of_a_card_gets_its_entry_and_no_new_one() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        assert_eq!(mesh.propose(card_change(2, 2)).await, Ok(after(at, 1)));
        assert_eq!(mesh.propose(card_change(2, 2)).await, Ok(after(at, 1)));
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(at, 2)));
    });
}

// The repeat waits on the write of the entry of the first call.
#[test]
fn a_repeat_returns_once_the_entry_of_the_first_is_on_disk() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        let mut calls = [
            Box::pin(mesh.propose(card_change(2, 2))),
            Box::pin(mesh.propose(card_change(2, 2))),
        ];
        assert!(poll_each(&mut calls).await.iter().all(Poll::is_pending));
        let [first, repeat] = calls;
        assert_eq!(repeat.await, Ok(after(at, 1)));
        assert_eq!(first.await, Ok(after(at, 1)));
        assert_eq!(mesh.propose(card_change(1, 2)).await, Ok(after(at, 2)));
    });
}

#[test]
fn a_leader_refuses_a_card_of_a_node_that_is_not_a_member_before_the_log() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        let stranger = Error::NotMember(key(2));
        let answer = mesh.answer(public(1), card_change(2, 2)).await;
        assert_eq!(answer, Err(stranger.clone()));
        assert_eq!(mesh.propose(card_change(2, 2)).await, Err(stranger));
        assert_eq!(mesh.propose(home_to(1)).await, Ok(after(at, 1)));
    });
}

// The node proposed a card of member 2 and then lost its lead, so another version of
// it gets `NotLeader` and the call forwards it.
#[test]
fn a_node_that_lost_its_lead_does_not_bound_a_card() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        let first = elect(&mesh).await;
        let mut proposal = pin!(mesh.propose(card_change(2, 2)));
        assert!(now(proposal.as_mut()).await.is_pending());
        let beat = Body::Append {
            prev: Position::default(),
            entries: vec![],
            commit: 0,
        };
        let later = common::proven_in(Term(first.term.0 + 1), 2, 1, beat);
        assert_eq!(mesh.receive(public(2), later), Ok(()));
        let leader = Some(key(2));
        let follower = Error::Raft(raft::Error::NotLeader { leader });
        assert_eq!(mesh.propose(card_change(2, 3)).await, Err(follower));
    });
}

// Node 2 replaces the entry of the card in a later term, and node 1 then leads again.
// The repeat proposes a new entry in its term.
#[test]
fn a_repeat_after_its_entry_was_replaced_proposes_again() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        let first = elect(&mesh).await;
        let card = after(first, 1);
        let mut proposal = pin!(mesh.propose(card_change(2, 2)));
        assert!(now(proposal.as_mut()).await.is_pending());
        let term = Term(first.term.0 + 1);
        let entries = (1..=card.index)
            .map(|index| Entry {
                at: Position { term, index },
                data: Data::Empty,
            })
            .collect();
        let replace = Body::Append {
            prev: Position::default(),
            entries,
            commit: 0,
        };
        let replace = common::proven_in(term, 2, 1, replace);
        assert_eq!(mesh.receive(public(2), replace), Ok(()));
        let second = elect(&mesh).await;
        assert!(second.term > term);
        let repeat = mesh.propose(card_change(2, 2)).await;
        assert_eq!(repeat.map(|at| at.term), Ok(second.term));
    });
}

#[test]
fn a_card_to_a_stopped_group_gives_stopped() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1, 2], &[1]).await.unwrap();
        let at = lead(&mesh, &node.clock(), home_to(1)).await;
        assert_eq!(mesh.propose(card_change(2, 2)).await, Ok(after(at, 1)));
        let stopped = Error::Stopped(fail_sync(&node));
        assert_eq!(mesh.propose(home_to(1)).await, Err(stopped.clone()));
        assert_eq!(mesh.propose(card_change(2, 2)).await, Err(stopped.clone()));
        assert_eq!(mesh.propose(card_change(2, 3)).await, Err(stopped));
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

// The run before signed version 2 with port 10 and stopped, and node 2 commits it
// after the call starts. The held card has the addresses of the call at version 1,
// so the call signs version 3, which applies after version 2.
#[test]
fn a_call_with_the_held_addresses_signs_above_a_version_of_the_file() {
    let before = async |config: Config| {
        let blocks = Blocks::new(Rc::clone(&config.pool)).unwrap();
        let signed = next(&config.files, &config.dir, &blocks, 1, false).await;
        assert_eq!(signed, Ok(Some(2)));
        config
    };
    let call = |_, mesh: Mesh| async move {
        let held = card_of(&mesh, 1);
        let set = mesh.set_addresses(held.addresses.clone()).await;
        (set, held, card_of(&mesh, 1))
    };
    let leader = |mut leader: Leader| async move {
        let (first, mut asked) = leader.proposal().await;
        leader.answer(&mut asked, at(2)).await.unwrap();
        let earlier = (encoded(&card_at(1, moved(1, 10), 2)), at(1));
        let each = vec![earlier, (encoded(&first), at(2))];
        leader.append_each(Position::default(), each).await;
        leader.rest(seconds(2)).await;
        card_in(&first).clone()
    };
    let ((set, held, card), first) = run_after(before, call, leader);
    assert_eq!(set, Ok(()));
    assert_eq!(held.version, 1);
    let expected = card::Card { version: 3, ..held };
    assert_eq!((first, card), (expected.clone(), expected));
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

// The leader puts the card of the call in its log and loses its lead before a
// quorum has it. The next leader replaces the entry, so the call forwards the card
// to it.
#[test]
fn a_leader_that_loses_its_lead_sets_its_addresses_through_the_next_leader() {
    let (mut cluster, old, ..) = Cluster::led(2);
    cluster.link_each(old, 1.0);
    cluster
        .board
        .lock()
        .unwrap()
        .addresses
        .insert(old, moved(old, 9));
    cluster.run(seconds(5));
    assert_eq!(cluster.board().moved, []);
    cluster.link_each(old, 0.0);
    cluster.run(seconds(5));
    let card = card::Card {
        addresses: moved(old, 9),
        version: 2,
        ..create_voter(old).card.card().clone()
    };
    assert_eq!(cluster.board().moved, [(old, Ok(()), card)]);
}

// The leader takes the forward, and the cut drops its answer. The call forwards
// the same card again after the heal, and gets the entry of the first.
#[test]
fn a_call_whose_answer_was_lost_returns_ok() {
    for step in 0..96_i64 {
        let (run, wait) = (u64::try_from(step / 12).unwrap(), step % 12);
        let (mut cluster, leader, follower, _) = Cluster::led(run);
        let config = link::Config {
            delay: Span::from_nanos(6 * TICK.nanos()),
            ..link::Config::default()
        };
        for other in IDS.into_iter().filter(|&id| id != follower) {
            let (a, b) = (cluster.node(follower).clone(), cluster.node(other).clone());
            cluster.sim.link(&a, &b, config);
            cluster.sim.link(&b, &a, config);
        }
        cluster.run(seconds(5));
        cluster
            .board
            .lock()
            .unwrap()
            .addresses
            .insert(follower, moved(follower, 9));
        cluster.run(Span::from_nanos((wait + 1) * TICK.nanos()));
        cluster.link_each(follower, 1.0);
        cluster.run(seconds(5));
        cluster.link_each(follower, 0.0);
        cluster.run(seconds(10));
        let card = card::Card {
            addresses: moved(follower, 9),
            version: 2,
            ..create_voter(follower).card.card().clone()
        };
        let moved = cluster.board().moved;
        assert_eq!(moved, [(follower, Ok(()), card.clone())], "step {step}");
        for id in IDS {
            let node = cluster.node(id).clone();
            cluster.sim.crash(&node, Crash::Process);
        }
        let node = cluster.node(leader).clone();
        let cards = stored(&mut cluster.sim, &node).into_iter().filter(|entry| {
            let Data::Bytes(bytes) = &entry.data else {
                return false;
            };
            let change = Change::decode(bytes).unwrap();
            matches!(change, Change::Card(_)) && card_in(&change).name == card.name
        });
        assert_eq!(cards.count(), 1, "step {step}");
    }
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

/// A call on another task waits for the turn of the file, and ends when it gets it.
#[test]
fn two_calls_on_two_tasks_both_end() {
    for seed in 0..8 {
        solo_at(seed, move |node, tasks| async move {
            let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
            lead(&mesh, &node.clock(), home_to(1)).await;
            let other = Rc::new(RefCell::new(None));
            let (into, second) = (Rc::clone(&other), mesh.clone());
            tasks.spawn(async move {
                *into.borrow_mut() = Some(second.set_addresses(moved(1, 10)).await);
            });
            let first = mesh.set_addresses(moved(1, 9)).await;
            node.clock().sleep(seconds(5)).await;
            let second = other.borrow_mut().take();
            let mut done = [Some(first), second];
            done.sort_by_key(|result| !matches!(result, Some(Ok(()))));
            let expected = [Some(Ok(())), Some(Err(Error::Soon))];
            assert_eq!(done, expected, "seed {seed}");
        });
    }
}

#[test]
fn set_addresses_gives_card_file_for_two_broken_slots() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
        lead(&mesh, &node.clock(), home_to(1)).await;
        assert_eq!(mesh.set_addresses(moved(1, 9)).await, Ok(()));
        let file = node
            .files()
            .open(Path::new("card"), files::Mode::Write)
            .await
            .unwrap();
        let broken = vec![0xff; 2 * files::SECTOR];
        let block = create_pool().copy(&broken).unwrap();
        file.write_at(0, &[block]).await.unwrap();
        file.sync().await.unwrap();
        file.close().await;
        let invalid = Error::CardFile {
            path: PathBuf::from("card"),
        };
        assert_eq!(mesh.set_addresses(moved(1, 9)).await, Err(invalid.clone()));
        assert_eq!(mesh.set_addresses(moved(1, 10)).await, Err(invalid));
    });
}

#[test]
fn set_addresses_gives_the_error_of_a_call_on_the_file() {
    for operation in [Operation::ReadAt, Operation::Sync] {
        solo(move |node, tasks| async move {
            let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
            lead(&mesh, &node.clock(), home_to(1)).await;
            node.fail_file(Path::new("card"), operation);
            let cause = files::Error::Io {
                path: PathBuf::from("card"),
                operation,
                code: 5,
            };
            let set = mesh.set_addresses(moved(1, 9)).await;
            assert_eq!(set, Err(Error::Files(cause)), "{operation}");
        });
    }
}

// A power cut at any point of a call leaves a mesh that opens and sets its addresses
// again, at a version above the held one.
#[test]
fn a_power_cut_in_a_call_leaves_a_mesh_that_sets_again() {
    for step in 0..96_i64 {
        let mut sim = Sim::new(common::simulation(u64::try_from(step).unwrap()));
        let node = sim.node(sim::node::Config::default());
        let shard = env::shards::Config {
            name: "call".into(),
            core: None,
        };
        let own = node.clone();
        let started = node.shards().start(shard, move |tasks| async move {
            let mesh = open(&own, &tasks, 1, &[1], &[1]).await.unwrap();
            lead(&mesh, &own.clock(), home_to(1)).await;
            mesh.set_addresses(moved(1, 9)).await.unwrap();
            pending::<()>().await;
        });
        drop(started.unwrap());
        sim.run_for(Span::from_nanos(step.saturating_mul(50_000_000)))
            .unwrap();
        sim.crash(&node, Crash::Power);
        sim.run_on(&node, move |node, tasks| async move {
            let mesh = open(&node, &tasks, 1, &[1], &[1]).await.unwrap();
            let before = card_of(&mesh, 1);
            lead(&mesh, &node.clock(), home_to(1)).await;
            assert_eq!(
                mesh.set_addresses(moved(1, 10)).await,
                Ok(()),
                "step {step}"
            );
            let after = card_of(&mesh, 1);
            assert_eq!(after.addresses, moved(1, 10), "step {step}");
            assert!(after.version > before.version, "step {step}");
        })
        .unwrap();
    }
}

// The largest card, of the most relay addresses of IPv6, goes through a forward.
#[test]
fn a_follower_sets_the_largest_card() {
    let (mut cluster, _, follower, _) = Cluster::led(0);
    let relay = |port| Address::Relay {
        node: public(9),
        at: SocketAddr::new(Ipv6Addr::new(0xfd00, 1, 2, 3, 4, 5, 6, 7).into(), port),
    };
    let count = u16::from(card::addresses::MAX);
    let largest = Addresses::new((0..count).map(relay).collect()).unwrap();
    cluster
        .board
        .lock()
        .unwrap()
        .addresses
        .insert(follower, largest.clone());
    cluster.run(seconds(5));
    let card = card::Card {
        addresses: largest,
        version: 2,
        ..create_voter(follower).card.card().clone()
    };
    assert_eq!(cluster.board().moved, [(follower, Ok(()), card)]);
}
