//! Tests of the file of the last version of a card of this node that the node signed.

use env::files::Mode;

use super::*;

use crate::driver::version::{FILE, next};
use crate::file;

/// What [`next`] gives on `node` for `held`.
fn next_on(sim: &mut Sim, node: &sim::node::Node, held: u64) -> u64 {
    tried(sim, node, held).unwrap()
}

/// What [`next`] gives on `node` for `held`, also an error.
#[expect(
    clippy::unwrap_in_result,
    reason = "a sim error is a test failure, not a file error"
)]
fn tried(sim: &mut Sim, node: &sim::node::Node, held: u64) -> Result<u64, Error> {
    sim.run_on(node, move |node, _| async move {
        let blocks = Blocks::new(create_pool()).unwrap();
        next(&node.files(), Path::new(""), &blocks, held).await
    })
    .unwrap()
}

/// A slot whose check holds over `format` and `version`.
fn slot(format: u16, version: u64) -> Vec<u8> {
    let mut bytes = file::frame(format, &version.to_le_bytes());
    bytes.resize(files::SECTOR, 0);
    bytes
}

/// Writes `bytes` at byte `at` of the file of `node`, and syncs it.
fn put(sim: &mut Sim, node: &sim::node::Node, at: u64, bytes: Vec<u8>) {
    sim.run_on(node, move |node, _| async move {
        let file = node
            .files()
            .open(Path::new(FILE), Mode::Write)
            .await
            .unwrap();
        let block = create_pool().copy(&bytes).unwrap();
        file.write_at(at, &[block]).await.unwrap();
        file.sync().await.unwrap();
    })
    .unwrap();
}

fn invalid() -> Error {
    Error::CardFile {
        path: PathBuf::from(FILE),
    }
}

/// Flips one bit of byte `at` of the file of node 1, and syncs it.
fn flip(sim: &mut Sim, node: &sim::node::Node, at: u64) {
    sim.run_on(node, move |node, _| async move {
        let file = node
            .files()
            .open(Path::new(FILE), Mode::Write)
            .await
            .unwrap();
        let pool = create_pool();
        let byte = file.read_at(at, pool.alloc(1).unwrap()).await.unwrap();
        let block = pool.copy(&[byte.first().unwrap() ^ 1]).unwrap();
        file.write_at(at, &[block]).await.unwrap();
        file.sync().await.unwrap();
    })
    .unwrap();
}

fn create_node() -> (Sim, sim::node::Node) {
    let mut sim = Sim::new(common::simulation(0));
    let node = sim.node(sim::node::Config::default());
    (sim, node)
}

#[test]
fn next_gives_a_version_above_the_held_one_and_above_each_it_gave() {
    let (mut sim, node) = create_node();
    assert_eq!(next_on(&mut sim, &node, 1), 2);
    assert_eq!(next_on(&mut sim, &node, 1), 3);
    assert_eq!(next_on(&mut sim, &node, 0), 4);
    assert_eq!(next_on(&mut sim, &node, 9), 10);
    assert_eq!(next_on(&mut sim, &node, 3), 11);
}

#[test]
fn a_power_cut_after_next_keeps_its_version() {
    let (mut sim, node) = create_node();
    assert_eq!(next_on(&mut sim, &node, 6), 7);
    sim.crash(&node, Crash::Power);
    assert_eq!(next_on(&mut sim, &node, 0), 8);
}

/// A slot whose check fails holds no version, so the other slot decides. Version 2 is
/// in the first slot and version 3 in the second.
#[test]
fn a_slot_with_a_broken_check_holds_no_version() {
    let sector = file::wide(files::SECTOR);
    for (at, expected) in [(sector, 3), (sector + 9, 3), (0, 4), (17, 4)] {
        let (mut sim, node) = create_node();
        next_on(&mut sim, &node, 1);
        next_on(&mut sim, &node, 1);
        flip(&mut sim, &node, at);
        assert_eq!(next_on(&mut sim, &node, 0), expected, "byte {at}");
    }
}

/// The write goes to the slot of the older version, so a slot that breaks after it
/// leaves the version that it wrote.
#[test]
fn next_writes_over_the_slot_of_the_older_version() {
    let sector = file::wide(files::SECTOR);
    let (mut sim, node) = create_node();
    for version in 2..=4 {
        assert_eq!(next_on(&mut sim, &node, 1), version);
    }
    // Version 4 went to the first slot, over version 2.
    flip(&mut sim, &node, sector);
    assert_eq!(next_on(&mut sim, &node, 0), 5);
    flip(&mut sim, &node, 0);
    assert_eq!(next_on(&mut sim, &node, 0), 6);
}

/// A power cut at any point of a call keeps each version that an earlier call gave, so
/// the next call gives a version above it.
#[test]
fn a_power_cut_in_next_keeps_each_version_given_before() {
    let (mut kept, mut lost) = (0_usize, 0_usize);
    for step in 0..96 {
        let mut sim = Sim::new(common::simulation(step));
        let node = sim.node(sim::node::Config::default());
        assert_eq!(next_on(&mut sim, &node, 1), 2);
        assert_eq!(next_on(&mut sim, &node, 1), 3);
        let shard = env::shards::Config {
            name: "next".into(),
            core: None,
        };
        let own = node.clone();
        let started = node.shards().start(shard, move |_| async move {
            let blocks = Blocks::new(create_pool()).unwrap();
            next(&own.files(), Path::new(""), &blocks, 1).await.unwrap();
        });
        drop(started.unwrap());
        let span = Span::from_nanos(i64::try_from(step).unwrap().saturating_mul(2_000));
        sim.run_for(span).unwrap();
        sim.crash(&node, Crash::Power);
        match next_on(&mut sim, &node, 0) {
            4 => lost = lost.saturating_add(1),
            5 => kept = kept.saturating_add(1),
            version => panic!("step {step} gave {version}"),
        }
    }
    assert_ne!(lost, 0, "no cut came before the write was durable");
    assert_ne!(kept, 0, "no cut came after the write was durable");
}

/// A call after a call that dropped waits for the calls that it left to run, and
/// does not find the file busy.
#[test]
fn next_after_a_dropped_call_gives_the_next_version() {
    let (mut sim, node) = create_node();
    let version = sim
        .run_on(&node, |node, _| async move {
            let blocks = Blocks::new(create_pool()).unwrap();
            let files = node.files();
            let mut dropped = Box::pin(next(&files, Path::new(""), &blocks, 1));
            let mut cx = Context::from_waker(Waker::noop());
            assert!(dropped.as_mut().poll(&mut cx).is_pending());
            drop(dropped);
            next(&files, Path::new(""), &blocks, 1).await.unwrap()
        })
        .unwrap();
    assert!((2..=3).contains(&version), "{version}");
}

/// A first write that broke leaves the other slot unwritten, so it is torn.
#[test]
fn a_broken_slot_beside_an_unwritten_one_holds_no_version() {
    let sector = file::wide(files::SECTOR);
    for at in [0, 17] {
        let (mut sim, node) = create_node();
        assert_eq!(next_on(&mut sim, &node, 4), 5);
        flip(&mut sim, &node, at);
        assert_eq!(next_on(&mut sim, &node, 1), 2, "byte {at}");
    }
    let (mut sim, node) = create_node();
    next_on(&mut sim, &node, 4);
    put(&mut sim, &node, 0, vec![0; files::SECTOR]);
    put(&mut sim, &node, sector, slot(1, 7));
    flip(&mut sim, &node, sector);
    assert_eq!(next_on(&mut sim, &node, 1), 2);
}

/// A write touches one slot, so two broken slots are not a torn write.
#[test]
fn two_broken_slots_give_card_file() {
    let sector = file::wide(files::SECTOR);
    let (mut sim, node) = create_node();
    next_on(&mut sim, &node, 1);
    next_on(&mut sim, &node, 1);
    flip(&mut sim, &node, 0);
    flip(&mut sim, &node, sector + 9);
    let error = tried(&mut sim, &node, 1).unwrap_err();
    assert_eq!(error, invalid());
    let text = "the file card holds no valid card version that this node signed";
    assert_eq!(error.to_string(), text);
}

/// A slot whose check holds with another format gives `CardFile`, beside a valid slot,
/// an unwritten one, or a broken one.
#[test]
fn a_slot_of_another_format_gives_card_file() {
    let sector = file::wide(files::SECTOR);
    for at in [0, sector] {
        for other in ["valid", "unwritten", "broken"] {
            let (mut sim, node) = create_node();
            next_on(&mut sim, &node, 1);
            let rest = if at == 0 { sector } else { 0 };
            match other {
                "valid" => put(&mut sim, &node, rest, slot(1, 3)),
                "unwritten" => put(&mut sim, &node, rest, vec![0; files::SECTOR]),
                _ => {
                    put(&mut sim, &node, rest, slot(1, 3));
                    flip(&mut sim, &node, rest);
                }
            }
            put(&mut sim, &node, at, slot(2, 3));
            let tried = tried(&mut sim, &node, 1);
            assert_eq!(tried, Err(invalid()), "byte {at}, {other}");
        }
    }
}
