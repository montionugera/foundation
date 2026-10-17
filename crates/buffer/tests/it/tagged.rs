//! The newest tagged entry of each key past the trims of a ring.

use super::*;

/// Commits of one frame each that pass the area of `AREA` three times.
const LAPS: u64 = 60;

/// A frame of index 9 at `seq` with one sample and 1000 bytes: a record of one
/// block.
fn frame(shard: &Shard, slot: Slot, seq: u64) -> Entry {
    let last = Some(i64::try_from(seq).expect("a small seq") + 1);
    entry(9, slot, Path::Live, seq, 1, last, shard.block(1000).into())
}

/// The shard of `shard` with trims on.
fn trimmed(shard: Shard) -> Shard {
    Shard {
        trimmed: true,
        ..shard
    }
}

/// Appends `batch` within 100 commit spans and waits for its commit.
async fn commit(shard: &Shard, buffer: &Buffer, batch: &[Entry]) {
    append_within(buffer, &shard.clock, batch, 100)
        .await
        .expect("the ring frees room");
    buffer.committed().await.expect("commits");
}

/// Ends `buffer` and opens the ring again.
async fn reopen(shard: &Shard, buffer: Buffer) -> (Buffer, Slots) {
    let ending = buffer.ended();
    drop(buffer);
    ending.await.expect("ends");
    let mut slots = Slots::new();
    let buffer = shard
        .open(layout(AREA, BODY_MAX), &mut slots)
        .await
        .expect("reopens");
    (buffer, slots)
}

/// Whether a trim passed the first record of index 9: a read from seq 0 gives a
/// gap.
async fn trimmed_past_the_start(buffer: &Buffer, slot: Slot) -> bool {
    let read = buffer
        .reader()
        .read(slot, Path::Live, mark(0, 0), usize::MAX)
        .await
        .expect("reads");
    read.gap.is_some()
}

/// A tag 1 entry, then frames over three laps of the ring: `newest` gives the same
/// bytes after each commit and after a reopen.
#[test]
fn keeps_the_newest_tagged_entry_past_each_trim() {
    run(400, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let (a, data) = (slots.index(key(1)), slots.index(key(9)));
        commit(
            &shard,
            &buffer,
            &[tagged(1, a, Path::Live, 0, 1, shard.block(300))],
        )
        .await;
        let expected = vec![(a, kept(&shard.block(300)))];
        for seq in 0..LAPS {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
            assert_eq!(newest(&buffer, Path::Live, 1), expected, "seq {seq}");
        }
        assert!(trimmed_past_the_start(&buffer, data).await);
        let (buffer, mut slots) = reopen(&shard, buffer).await;
        let a = slots.index(key(1));
        assert_eq!(
            newest(&buffer, Path::Live, 1),
            vec![(a, kept(&shard.block(300)))]
        );
    });
}

/// After a lap carries a tag 1 entry, a newer one of the same key replaces it,
/// past the next lap and after a reopen.
#[test]
fn a_newer_tagged_entry_replaces_a_carried_one() {
    run(401, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let (a, data) = (slots.index(key(1)), slots.index(key(9)));
        commit(
            &shard,
            &buffer,
            &[tagged(1, a, Path::Live, 0, 1, shard.block(300))],
        )
        .await;
        for seq in 0..LAPS / 3 * 2 {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
        }
        assert!(trimmed_past_the_start(&buffer, data).await);
        commit(
            &shard,
            &buffer,
            &[tagged(1, a, Path::Live, 0, 1, shard.block(301))],
        )
        .await;
        let expected = vec![(a, kept(&shard.block(301)))];
        assert_eq!(newest(&buffer, Path::Live, 1), expected);
        for seq in LAPS / 3 * 2..LAPS {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
            assert_eq!(newest(&buffer, Path::Live, 1), expected, "seq {seq}");
        }
        let (buffer, mut slots) = reopen(&shard, buffer).await;
        let a = slots.index(key(1));
        assert_eq!(
            newest(&buffer, Path::Live, 1),
            vec![(a, kept(&shard.block(301)))]
        );
    });
}

/// The newer entry of a key comes with the frame of each commit in turn, so in one
/// of them the commit carries the older copy after the newer entry. `newest` gives
/// the newer entry after each later commit and after a reopen, which walks the
/// carried copy after it. It fails when a copy replaces the copy of a later mark.
#[test]
fn a_carried_copy_never_replaces_a_newer_entry() {
    for newer in 0..LAPS / 3 * 2 {
        run(402, move |shard| async move {
            let shard = trimmed(shard);
            let mut slots = Slots::new();
            let buffer = shard
                .open(layout(AREA, BODY_MAX), &mut slots)
                .await
                .expect("opens");
            let (a, data) = (slots.index(key(1)), slots.index(key(9)));
            let old = tagged(1, a, Path::Live, 0, 1, shard.block(300));
            commit(&shard, &buffer, &[old]).await;
            let expected = vec![(a, kept(&shard.block(301)))];
            for seq in 0..LAPS {
                let mut batch = vec![frame(&shard, data, seq)];
                if seq == newer {
                    batch.push(tagged(1, a, Path::Live, 0, 1, shard.block(301)));
                }
                commit(&shard, &buffer, &batch).await;
                if seq >= newer {
                    let found = newest(&buffer, Path::Live, 1);
                    assert_eq!(found, expected, "newer at {newer}, seq {seq}");
                }
            }
            let (buffer, mut slots) = reopen(&shard, buffer).await;
            let a = slots.index(key(1));
            let found = newest(&buffer, Path::Live, 1);
            assert_eq!(
                found,
                vec![(a, kept(&shard.block(301)))],
                "newer at {newer}"
            );
        });
    }
}

/// A read of the path of a carried entry from seq 0 after three laps gives the gap
/// of the trims and then the frames only, never the carried entry.
#[test]
fn a_read_never_gives_a_carried_entry() {
    run(403, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let data = slots.index(key(9));
        let state = tagged(9, data, Path::Live, 0, 1, shard.block(300));
        commit(&shard, &buffer, &[state]).await;
        for seq in 0..LAPS {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
        }
        assert_eq!(
            newest(&buffer, Path::Live, 1),
            vec![(data, kept(&shard.block(300)))]
        );
        let read = buffer
            .reader()
            .read(data, Path::Live, mark(0, 0), usize::MAX)
            .await
            .expect("reads");
        let gap = read.gap.clone().expect("a trim passed seq 0");
        assert_eq!(gap.start, 0);
        let firsts: Vec<_> = read.entries.iter().map(|stored| stored.first).collect();
        assert_eq!(firsts, (gap.end..LAPS).collect::<Vec<_>>());
        assert!(
            read.entries.iter().all(|stored| stored.tag == 0),
            "{read:?}"
        );
        assert_eq!(read.next, mark(LAPS, 0));
    });
}

/// Tags 1 and 2 on the live path of index 1, tag 1 on its backfill path, and tag 1
/// on the live path of index 2, past three laps and a reopen: each tag of each path
/// keeps its own entry.
#[test]
fn keeps_each_tag_of_each_path_apart() {
    run(404, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let (a, b, data) = (
            slots.index(key(1)),
            slots.index(key(2)),
            slots.index(key(9)),
        );
        let states = [
            tagged(1, a, Path::Live, 0, 1, shard.block(301)),
            tagged(1, a, Path::Live, 0, 2, shard.block(302)),
            tagged(1, a, Path::Backfill, 0, 1, shard.block(303)),
            tagged(2, b, Path::Live, 0, 1, shard.block(304)),
        ];
        commit(&shard, &buffer, &states).await;
        for seq in 0..LAPS {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
        }
        assert!(trimmed_past_the_start(&buffer, data).await);
        let each = |buffer: &Buffer, [a, b]: [Slot; 2]| {
            assert_eq!(
                newest(buffer, Path::Live, 1),
                vec![(a, kept(&shard.block(301))), (b, kept(&shard.block(304)))]
            );
            assert_eq!(
                newest(buffer, Path::Live, 2),
                vec![(a, kept(&shard.block(302)))]
            );
            assert_eq!(
                newest(buffer, Path::Backfill, 1),
                vec![(a, kept(&shard.block(303)))]
            );
            assert_eq!(newest(buffer, Path::Backfill, 2), Vec::new());
            assert_eq!(newest(buffer, Path::Live, 3), Vec::new());
        };
        each(&buffer, [a, b]);
        let (buffer, mut slots) = reopen(&shard, buffer).await;
        each(&buffer, [slots.index(key(1)), slots.index(key(2))]);
    });
}

/// A power cut at each point of three laps of frames, with a newer tagged entry of
/// the key after one lap: the reopen gives the newest committed entry, or the one
/// whose commit the cut stopped, never none and never an older one.
#[test]
fn a_crash_in_the_commit_that_carries_keeps_the_entry() {
    each_cut(0..2, 2_500_000, |seed, cut| {
        let (mut sim, node) = create_node(seed);
        let ended = Arc::new(AtomicBool::new(false));
        let (flag, own) = (Arc::clone(&ended), node.clone());
        // The lens of the newest committed tagged entry and of the newest appended.
        let lens = Arc::new(Mutex::new((0, 0)));
        let given = Arc::clone(&lens);
        drop(on_node(&node, "cut", move |tasks| async move {
            let shard = trimmed(shard_on(own, tasks));
            let mut slots = Slots::new();
            let buffer = shard
                .open(layout(AREA, BODY_MAX), &mut slots)
                .await
                .expect("opens");
            let (a, data) = (slots.index(key(1)), slots.index(key(9)));
            for (seq, len) in (0..LAPS).zip(300..) {
                let mut batch = vec![frame(&shard, data, seq)];
                if seq == 0 || seq == LAPS / 3 {
                    batch.push(tagged(1, a, Path::Live, 0, 1, shard.block(len)));
                    given.lock().expect("no panic").1 = len;
                }
                commit(&shard, &buffer, &batch).await;
                let mut lens = given.lock().expect("no panic");
                lens.0 = lens.1;
            }
            flag.store(true, Ordering::Relaxed);
            std::future::pending::<()>().await;
        }));
        sim.run_for(Span::from_nanos(cut)).expect("the run goes on");
        sim.crash(&node, sim::Crash::Power);
        let (durable, queued) = *lens.lock().expect("no panic");
        let found = sim
            .run_on(&node, move |node, tasks| async move {
                let shard = trimmed(shard_on(node, tasks));
                let mut slots = Slots::new();
                let buffer = shard
                    .open(layout(AREA, BODY_MAX), &mut slots)
                    .await
                    .expect("reopens");
                let a = slots.index(key(1));
                let found = newest(&buffer, Path::Live, 1);
                let block = |len| (len > 0).then(|| vec![(a, kept(&shard.block(len)))]);
                (found, block(durable), block(queued))
            })
            .expect("the reopen ends");
        let (found, durable, queued) = found;
        let case = format!("seed {seed}, cut at {cut} ns");
        if let Some(durable) = durable {
            assert!(
                found == durable || Some(&found) == queued.as_ref(),
                "{case}"
            );
        } else {
            assert!(
                found.is_empty() || Some(&found) == queued.as_ref(),
                "{case}"
            );
        }
        ended.load(Ordering::Relaxed)
    });
}

/// Tagged entries of 4000 bytes of new indexes, one in each commit, until the ring
/// has no room: then a frame gets `Full` too, and `newest` gives each entry, also
/// after a reopen.
#[test]
fn tagged_state_over_the_area_refuses_data_and_keeps_each_entry() {
    run(405, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let mut expected = Vec::new();
        for index in 1..=u32::try_from(AREA / BLOCK).expect("few blocks") {
            let slot = slots.index(key(index));
            let state = tagged(index, slot, Path::Live, 0, 1, shard.block(4000));
            match append_within(&buffer, &shard.clock, &[state], 100).await {
                Ok(()) => buffer.committed().await.expect("commits"),
                Err(Rejected::Full { .. }) => break,
                Err(refused) => panic!("index {index}: {refused}"),
            }
            expected.push((slot, kept(&shard.block(4000))));
        }
        assert!(expected.len() < to_usize(AREA / BLOCK), "the ring fills");
        let data = slots.index(key(99));
        let refused =
            append_within(&buffer, &shard.clock, &[frame(&shard, data, 0)], 100).await;
        assert!(matches!(refused, Err(Rejected::Full { .. })), "{refused:?}");
        assert_eq!(newest(&buffer, Path::Live, 1), expected);
        let count = u32::try_from(expected.len()).expect("few entries");
        let (buffer, mut slots) = reopen(&shard, buffer).await;
        let mut reopened: Vec<_> = (1..=count)
            .map(|index| (slots.index(key(index)), kept(&shard.block(4000))))
            .collect();
        reopened.sort_unstable();
        assert_eq!(newest(&buffer, Path::Live, 1), reopened);
    });
}

/// Makes a ring of `AREA` whose oldest record, at `BLOCK`, holds only a tag 1 entry
/// of index 1, with a data record of index 2 after it and a carry record of the
/// tails of both in each block after them, and none free. The record at 0 is a
/// carry record that follows the last, so that the ring has no free block.
async fn create_tagged_full_ring(shard: &Shard) {
    let mut slots = Slots::new();
    let buffer = shard
        .open(layout(AREA, BODY_MAX), &mut slots)
        .await
        .expect("opens");
    let (a, b) = (slots.index(key(1)), slots.index(key(2)));
    for appended in [
        tagged(1, a, Path::Live, 0, 1, shard.block(10)),
        entry(2, b, Path::Live, 0, 2, Some(40), Parts::default()),
    ] {
        buffer.append([appended]).expect("queues");
        buffer.committed().await.expect("commits");
    }
    drop(buffer);
    let tails = [carried(1, 0, 1, None), carried(2, 2, 0, Some(40))];
    let body = carry_body(&tails);
    for block in 3..AREA / BLOCK {
        shard.put_carry(block * BLOCK, &body).await;
    }
    let file = shard.bytes().await;
    let at = |offset: u64| to_usize(AREA_START + offset);
    let u32_at = |at: usize| {
        u32::from_le_bytes(file[at..at + 4].try_into().expect("four bytes"))
    };
    let restart = u32_at(at(0) + 9);
    let chain = u32_at(at(AREA - BLOCK) + 4);
    let len = u32::try_from(body.len()).expect("a short body");
    let mut record = [&len.to_le_bytes()[..], &[0; 4], &[CARRY], &body].concat();
    let crc = crc32c::crc32c_append(chain, &record[..4]);
    let crc = crc32c::crc32c_append(crc, &record[8..]);
    record[4..8].copy_from_slice(&crc.to_le_bytes());
    shard.put(at(0), &record).await;
    shard.tamper(TAIL_AT, &BLOCK.to_le_bytes()).await;
    shard.tamper(CHAIN_AT, &restart.to_le_bytes()).await;
}

/// An open of a ring with no free block, whose oldest record holds the only copy
/// of the newest tag 1 entry of index 1, frees nothing and gives `Full`.
#[test]
fn an_open_whose_oldest_record_holds_a_tagged_entry_is_full() {
    run(406, |shard| async move {
        let shard = trimmed(shard);
        create_tagged_full_ring(&shard).await;
        let full = Error::Full {
            needed: 4096,
            free: 0,
        };
        shard
            .open_refused(layout(AREA, BODY_MAX), full, "a tagged holder")
            .await;
    });
}
