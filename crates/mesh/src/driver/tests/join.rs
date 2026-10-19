//! Tests of the answer of a voter to a join request.

use transport::Class;

use super::founding::add_subject;
use super::serve::{MALFORMED, Peer, REFUSED, codes, next, run, stopped};
use super::*;
use crate::driver::founding::FILE;
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

// Both calls propose before the first entry applies, so the entry of the second
// applies after the join and the region refuses it.
#[test]
fn a_join_that_another_call_applied_first_is_admitted() {
    solo(|node, tasks| async move {
        let mesh = recorded(&node, &tasks, &[ticket_of(8, "plant", false, LAST)]).await;
        let time = synced(&node);
        let mut calls = [1, 2]
            .map(|_| Box::pin(mesh.admit(public(4), &time, request(4, 8, &["disk"]))));
        let polls = poll_each(&mut calls).await;
        assert!(polls.iter().all(Poll::is_pending));
        let [first, second] = calls;
        assert_eq!(first.await, Ok(Ok(())));
        assert_eq!(second.await, Ok(Ok(())));
        assert_eq!(mesh.member(key(4)).unwrap().card, common::member(4).card);
    });
}

// The second call waits for the read of the first, so the file opens once.
#[test]
fn two_answers_at_one_time_share_one_copy_and_none_stays_after_the_last() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        let mut calls = [1, 2].map(|_| Box::pin(mesh.founding.get()));
        let polls = poll_each(&mut calls).await;
        assert!(polls.iter().all(Poll::is_pending));
        let [first, second] = calls;
        let (first, second) = (first.await.unwrap(), second.await.unwrap());
        assert!(Rc::ptr_eq(&first, &second));
        assert!(region::Founding::decode(first.body()).is_some());
        let copy = Rc::downgrade(&first);
        drop((first, second));
        assert!(copy.upgrade().is_none());
        let again = mesh.founding.get().await.unwrap();
        assert!(region::Founding::decode(again.body()).is_some());
    });
}

// An answer whose stream fails drops its call in the read.
#[test]
fn a_call_that_drops_in_its_read_ends_its_turn() {
    solo(|node, tasks| async move {
        let mesh = open(&node, &tasks, 1, &IDS, &IDS).await.unwrap();
        let mut calls = [1, 2].map(|_| Box::pin(mesh.founding.get()));
        let polls = poll_each(&mut calls).await;
        assert!(polls.iter().all(Poll::is_pending));
        let [first, second] = calls;
        drop(first);
        let copy = second.await.unwrap();
        assert!(region::Founding::decode(copy.body()).is_some());
    });
}

/// The join request of node 4 by ticket `ticket`.
fn join(ticket: u8) -> Vec<u8> {
    let request = Box::new(request(4, ticket, &[]));
    Message::Join { request }.encode()
}

/// Sends `messages` on a join stream, then ends it, and gives each message of the
/// answer.
async fn ask_to_join(peer: Peer, messages: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let (mut sender, mut receiver) = peer.open(Class::Complete, messages).await;
    sender.finish().unwrap();
    let mut answer = Vec::new();
    while let Some(message) = receiver.recv().await.unwrap() {
        answer.push(message.to_vec());
    }
    answer
}

// The founding is over two messages, so each part but the last fills one.
#[test]
fn serve_admits_a_join_and_sends_the_founding_in_parts() {
    let ((served, member, founding), answer) = run(
        |node, tasks, incoming| async move {
            let mut config = config(&node, &tasks, 1, &[1], &[1]).await;
            add_subject(&mut config.founding, 5_000);
            let founding = config.founding.encode();
            let tickets = [ticket_of(8, "plant", true, LAST)];
            let mesh = recorded_with(&node, config, &tickets).await;
            let served = mesh.serve(public(4), incoming).await;
            (served, mesh.member(key(4)), founding)
        },
        |peer| async move { ask_to_join(peer, &[join(8)]).await },
    );
    assert_eq!(served, Ok(()));
    assert_eq!(member.unwrap().card, common::member(4).card);
    let (joined, parts) = answer.split_first().unwrap();
    let bytes = founding.len();
    assert_eq!(Message::decode(joined), Some(Message::Joined { bytes }));
    let lengths: Vec<_> = parts.iter().map(Vec::len).collect();
    let last = bytes - 2 * (BYTES_MAX - 1) + 1;
    assert_eq!(lengths, [BYTES_MAX, BYTES_MAX, last]);
    let mut sent = Vec::new();
    for part in parts {
        let Some(Message::Founding { part }) = Message::decode(part) else {
            panic!("not a part");
        };
        sent.extend(part);
    }
    assert!(sent == founding, "the parts are not the founding");
}

#[test]
fn serve_answers_a_join_that_it_does_not_admit_with_the_refusal() {
    let unknown = Refusal::Refused(region::Refused::Unknown {
        public_key: public(9),
    });
    for (voter, ticket, refusal) in [(1, 9, unknown), (2, 8, Refusal::NoVote)] {
        let (served, answer) = run(
            move |node, tasks, incoming| async move {
                let mesh = if voter == 1 {
                    recorded(&node, &tasks, &[ticket_of(8, "plant", true, LAST)]).await
                } else {
                    open(&node, &tasks, voter, &IDS, &[1]).await.unwrap()
                };
                mesh.serve(public(4), incoming).await
            },
            move |peer| async move { ask_to_join(peer, &[join(ticket)]).await },
        );
        assert_eq!(served, Ok(()));
        assert_eq!(answer, [Message::Unjoined { refusal }.encode()]);
    }
}

#[test]
fn serve_stops_a_join_of_a_card_that_the_peer_does_not_hold() {
    let ((served, member), seen) = run(
        |node, tasks, incoming| async move {
            let tickets = [ticket_of(8, "plant", true, LAST)];
            let mesh = recorded(&node, &tasks, &tickets).await;
            let served = mesh.serve(public(5), incoming).await;
            (served, mesh.member(key(4)))
        },
        |peer| async move { stopped(peer, &[join(8)]).await },
    );
    assert_eq!(
        (served, member),
        (Err(Error::Spoofed { from: key(4) }), None)
    );
    assert_eq!(seen, codes(REFUSED));
}

// The answer is on the stream when the second message comes.
#[test]
fn serve_stops_a_join_stream_at_a_second_message() {
    let (served, (answer, finished)) = run(
        |node, tasks, incoming| async move {
            let tickets = [ticket_of(8, "plant", true, LAST)];
            let mesh = recorded(&node, &tasks, &tickets).await;
            mesh.serve(public(4), incoming).await
        },
        |peer| async move {
            let (mut sender, mut receiver) =
                peer.open(Class::Complete, &[join(9), join(9)]).await;
            let answer = next(&mut receiver).await;
            peer.settle().await;
            (answer, sender.finish())
        },
    );
    let unknown = Refusal::Refused(region::Refused::Unknown {
        public_key: public(9),
    });
    assert_eq!(served, Err(Error::Malformed));
    assert_eq!(answer, Ok(Some(Message::Unjoined { refusal: unknown })));
    let stopped = transport::Error::Stopped { code: MALFORMED };
    assert_eq!(finished, Err(stopped));
}

// The node is a member once the voter admits it, so the failure is no refusal, and
// the node can ask again.
#[test]
fn serve_gives_no_code_of_the_mesh_when_it_cannot_read_the_founding() {
    let ((served, member), seen) = run(
        |node, tasks, incoming| async move {
            let tickets = [ticket_of(8, "plant", true, LAST)];
            let mesh = recorded(&node, &tasks, &tickets).await;
            node.files().remove(Path::new(FILE)).await.unwrap();
            let served = mesh.serve(public(4), incoming).await;
            (served, mesh.member(key(4)).is_some())
        },
        |peer| async move { stopped(peer, &[join(8)]).await },
    );
    let path = PathBuf::from(FILE);
    assert_eq!((served, member), (Err(Error::Unfounded { path }), true));
    assert_eq!(seen, codes(Code(0)));
}
