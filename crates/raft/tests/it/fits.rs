//! A node sends no message that `Config::fits` refuses. An append with no proof
//! drops its last entries, down to one; a message with a proof drops its entries,
//! then the oldest links of its chain.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use proptest::prelude::*;
use raft::{
    Answer, Body, Change, Config, Data, Entry, Grant, Hard, Link, Message, Position,
    Proof, Raft, Role, Start, Term, Voters,
};
use types::node;

use crate::network::{Network, run, run_of_many};

const ELECTION: u32 = 10;

/// The bytes of `message` in a test byte form near that of `mesh`, with 64 bytes for
/// each signature, present or not.
pub(crate) fn bytes(message: &Message) -> usize {
    let proof = message.proof.as_ref().map_or(0, proof);
    let chain: usize = message.chain.iter().map(link).sum();
    41 + 1 + proof + 8 + chain + body(&message.body)
}

/// The bytes of `link`: its position, then its change.
pub(crate) fn link(link: &Link) -> usize {
    16 + change(&link.change)
}

fn proof(proof: &Proof) -> usize {
    1 + 16 + 8 + proof.voters.len() * (16 + 64)
}

fn change(change: &Change) -> usize {
    let keys = |keys: &BTreeSet<node::Key>| 8 + keys.len() * 16;
    let voters = &change.voters;
    keys(&voters.incoming) + keys(&voters.outgoing) + proof(&change.votes) + 64
}

fn body(body: &Body) -> usize {
    let answer = |answer: &Answer| match answer {
        Answer::Refused => 1,
        Answer::Granted(_) => 1 + 64,
    };
    1 + match body {
        Body::PreVote { .. } | Body::Vote { .. } => 16,
        Body::PreVoteReply { answer: a } | Body::VoteReply { answer: a } => answer(a),
        Body::HeartbeatReply => 0,
        Body::Heartbeat { .. }
        | Body::AppendReply { .. }
        | Body::AppendReject { .. } => 8,
        Body::Append { entries, .. } => {
            16 + 8 + entries.iter().map(entry).sum::<usize>()
        }
    }
}

fn entry(entry: &Entry) -> usize {
    16 + 1
        + match &entry.data {
            Data::Empty => 0,
            Data::Bytes(bytes) => 8 + bytes.len(),
            Data::Voters(change) => self::change(change),
        }
}

/// Whether `message` is at most 1472 bytes.
fn fits_1472(message: &Message) -> bool {
    bytes(message) <= 1472
}

/// Whether `message` with no link is at most 800 bytes: an append of 5 voters holds
/// a few entries, and the chain of a node far behind is whole.
pub(crate) fn fits_800(message: &Message) -> bool {
    let chain: usize = message.chain.iter().map(link).sum();
    bytes(message) - chain <= 800
}

fn key(id: u8) -> node::Key {
    node::Key::from_u128(u128::from(id))
}

fn set(ids: &[u8]) -> BTreeSet<node::Key> {
    ids.iter().copied().map(key).collect()
}

fn node(id: u8) -> Raft {
    let config = Config {
        key: key(id),
        election_ticks: ELECTION,
        heartbeat_ticks: 1,
        fits: fits_1472,
    };
    let start = Start {
        hard: Hard::default(),
        voters: Voters {
            incoming: set(&[1, 2, 3]),
            outgoing: BTreeSet::new(),
        },
        entries: Vec::new(),
        applied: 0,
    };
    Raft::new(config, start).unwrap()
}

// What a run sent, each message of which fits, and what each node committed.
#[derive(Default)]
struct Ran {
    sent: Vec<Message>,
    committed: BTreeMap<node::Key, Vec<Entry>>,
}

impl Ran {
    // Takes the `Ready` of `raft` into the run, and its messages into `queue`.
    fn take(&mut self, raft: &mut Raft, queue: &mut VecDeque<Message>) {
        let ready = raft.ready();
        let committed = self.committed.entry(raft.key()).or_default();
        committed.extend(ready.committed);
        for message in ready.messages {
            assert!(
                fits_1472(&message),
                "{} bytes: {message:?}",
                bytes(&message)
            );
            self.sent.push(message.clone());
            queue.push_back(message);
        }
    }
}

// Delivers every message between the nodes of `side` until none remains; the rest
// are lost.
fn run_among(nodes: &mut BTreeMap<node::Key, Raft>, side: &[u8], ran: &mut Ran) {
    let side = set(side);
    let mut queue = VecDeque::new();
    for id in &side {
        ran.take(nodes.get_mut(id).unwrap(), &mut queue);
    }
    while let Some(message) = queue.pop_front() {
        let Some(raft) = nodes
            .get_mut(&message.to)
            .filter(|_| side.contains(&message.to))
        else {
            continue;
        };
        raft.step(message).unwrap();
        ran.take(raft, &mut queue);
    }
}

// Ticks the nodes of `side` until one of them leads, and returns it.
fn elect(nodes: &mut BTreeMap<node::Key, Raft>, side: &[u8], ran: &mut Ran) -> u8 {
    for _ in 0..4 * ELECTION {
        for &id in side {
            nodes.get_mut(&key(id)).unwrap().tick(u64::from(id));
        }
        run_among(nodes, side, ran);
        let leads = |id: &&u8| nodes[&key(**id)].role() == Role::Leader;
        if let Some(&id) = side.iter().find(leads) {
            return id;
        }
    }
    panic!("no node of {side:?} leads");
}

// The voters other than `id`.
fn others(id: u8) -> Vec<u8> {
    [1, 2, 3].into_iter().filter(|&other| other != id).collect()
}

#[test]
fn a_link_of_three_voters_in_each_set_is_457_bytes() {
    let link = Link {
        at: Position {
            term: Term(1),
            index: 1,
        },
        change: Change {
            voters: Voters {
                incoming: set(&[1, 2, 3]),
                outgoing: set(&[1, 2, 3]),
            },
            votes: Proof {
                grant: Grant::Vote,
                candidate: key(1),
                voters: [1, 2, 3].map(|id| (key(id), None)).into(),
            },
            signature: None,
        },
    };
    assert_eq!(self::link(&link), 457);
}

#[test]
fn three_voters_elect_a_leader_after_a_change_in_each_of_3_terms() {
    let mut nodes: BTreeMap<node::Key, Raft> =
        (1..=3).map(|id| (key(id), node(id))).collect();
    let ran = &mut Ran::default();
    nodes.get_mut(&key(1)).unwrap().campaign();
    run_among(&mut nodes, &[1, 2, 3], ran);
    let mut leader = 1;
    for _ in 0..3 {
        let raft = nodes.get_mut(&key(leader)).unwrap();
        raft.propose_voters(set(&[1, 2, 3])).unwrap();
        run_among(&mut nodes, &[1, 2, 3], ran);
        leader = elect(&mut nodes, &others(leader), ran);
        run_among(&mut nodes, &[1, 2, 3], ran);
    }
    let links: Vec<Position> = ran.committed[&key(1)]
        .iter()
        .filter(|entry| matches!(entry.data, Data::Voters(_)))
        .map(|entry| entry.at)
        .collect();
    assert_eq!(links.len(), 6);

    let ran = &mut Ran::default();
    let elected = elect(&mut nodes, &others(leader), ran);
    let term = nodes[&key(elected)].term();
    let vote = ran
        .sent
        .iter()
        .find(|m| m.term == term && matches!(m.body, Body::Vote { .. }))
        .unwrap();
    let chain: Vec<Position> = vote.chain.iter().map(|link| link.at).collect();
    assert!(chain.len() < links.len(), "{chain:?}");
    assert!(links.ends_with(&chain), "{chain:?} of {links:?}");

    let at = nodes
        .get_mut(&key(elected))
        .unwrap()
        .propose(vec![7])
        .unwrap();
    run_among(&mut nodes, &[1, 2, 3], ran);
    // A heartbeat gives the followers the commit.
    nodes.get_mut(&key(elected)).unwrap().tick(0);
    run_among(&mut nodes, &[1, 2, 3], ran);
    for id in [1, 2, 3] {
        let committed = &ran.committed[&key(id)];
        assert_eq!(
            committed.last().map(|entry| entry.at),
            Some(at),
            "node {id}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // `Network` checks that each message fits, and that an append with no proof
    // carries an entry when the leader has one to send.
    #[test]
    fn each_message_of_a_run_with_changes_fits((logs, actions) in run()) {
        let mut network = Network::with_fits(&logs, 0, fits_800);
        for action in &actions {
            network.apply(action);
        }
    }

    #[test]
    fn a_proposal_to_the_leader_of_a_mended_network_of_small_messages_is_applied(
        (logs, actions) in run_of_many(),
        random in any::<u64>(),
    ) {
        let mut network = Network::with_fits(&logs, random, fits_800);
        let (leader, _) = network.settle(&actions)?;
        let at = network.propose(leader).unwrap();
        let nodes: Vec<usize> = (0..network.nodes.len()).collect();
        network.apply_until(&nodes, at.index);
        for disk in &network.disks {
            prop_assert!(disk.applied >= at.index, "applied {}", disk.applied);
        }
    }
}

const CASES: u32 = 1000;
