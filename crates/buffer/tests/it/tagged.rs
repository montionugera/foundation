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

/// Tagged entries of 2000 bytes of new indexes, one in each commit, until the ring
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
            let state = tagged(index, slot, Path::Live, 0, 1, shard.block(2000));
            match append_within(&buffer, &shard.clock, &[state], 100).await {
                Ok(()) => buffer.committed().await.expect("commits"),
                Err(Rejected::Full { .. }) => break,
                Err(refused) => panic!("index {index}: {refused}"),
            }
            expected.push((slot, kept(&shard.block(2000))));
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
            .map(|index| (slots.index(key(index)), kept(&shard.block(2000))))
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

/// Two tagged entries of each of 3000 indexes, in records of 1000 entries, then
/// commits of 100 frames over two laps: each commit gets room, and `newest` gives
/// each entry.
#[test]
fn dense_tagged_state_on_a_trimmed_ring_never_stays_full() {
    run(407, |shard| async move {
        let shard = trimmed(shard);
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(1024 * BLOCK, 32 * BODY_MAX + 279), &mut slots)
            .await
            .expect("opens");
        let mut expected = [Vec::new(), Vec::new()];
        for indexes in (1..=3000).collect::<Vec<u32>>().chunks(500) {
            for &index in indexes {
                let slot = slots.index(key(index));
                let mut pair = Vec::new();
                for (tag, len) in [(1, 17), (2, 73)] {
                    pair.push(tagged(
                        index,
                        slot,
                        Path::Live,
                        0,
                        tag,
                        shard.block(len),
                    ));
                    expected[usize::from(tag - 1)]
                        .push((slot, kept(&shard.block(len))));
                }
                append_within(&buffer, &shard.clock, &pair, 100)
                    .await
                    .expect("the ring frees room");
            }
            buffer.committed().await.expect("commits");
        }
        let data = slots.index(key(9));
        for round in 0..100 {
            let batch: Vec<_> = (round * 100..(round + 1) * 100)
                .map(|seq| frame(&shard, data, seq))
                .collect();
            commit(&shard, &buffer, &batch).await;
        }
        assert!(trimmed_past_the_start(&buffer, data).await);
        assert_eq!(newest(&buffer, Path::Live, 1), expected[0]);
        assert_eq!(newest(&buffer, Path::Live, 2), expected[1]);
    });
}

/// The shape of a workload of tagged state and data on a ring with trims.
#[derive(Clone, Debug)]
struct Shape {
    /// Bodies of at most this many blocks less the record header.
    blocks: usize,
    /// Bytes of each tagged entry.
    len: usize,
    /// Tagged entries of new indexes in each batch, at most what one record holds.
    per: usize,
    /// Batches of tagged entries in each commit.
    batches: usize,
    /// Bytes of each frame, one frame to a record.
    frame: usize,
    /// Frames in each commit.
    frames: usize,
    /// Entries with no bytes of new indexes, in one record after the tagged
    /// entries, whose tails then stay idle.
    idle: usize,
}

/// Bytes of the ring of [`lives_with_holders_under_a_quarter_of_the_area`].
const LIVE_AREA: u64 = 64 * BLOCK;

/// Bytes of the carry body of `idle` tails, and of `n` tails and `n` entries of
/// `len` bytes.
fn carried_len(idle: usize, n: usize, len: usize) -> usize {
    4 + idle * 42 + n * (42 + 51 + len)
}

fn shape() -> impl Strategy<Value = Shape> {
    let lens = prop_oneof![1..=64_usize, 1..=Layout::ENTRY_MAX_MIN];
    let idle = prop_oneof![Just(0_usize), 0..=320_usize];
    (
        2..=4_usize,
        lens,
        1..=64_usize,
        1..=4_usize,
        1..=16_usize,
        idle,
    )
        .prop_flat_map(|(blocks, len, per, batches, frames, idle)| {
            let body_max = blocks * to_usize(BLOCK) - 9;
            let per = per.min((body_max - 4) / (42 + 51 + len)).max(1);
            let idle = idle.min((body_max - 4) / 51);
            (
                Just((blocks, len, per, batches, frames, idle)),
                1000..=body_max - 55,
            )
        })
        .prop_map(|((blocks, len, per, batches, frames, idle), frame)| {
            let block = to_usize(BLOCK);
            let frames =
                frames.min(to_usize(LIVE_AREA) / 4 / frame.next_multiple_of(block));
            Shape {
                blocks,
                len,
                per,
                batches,
                frame,
                frames: frames.max(1),
                idle,
            }
        })
}

/// Tagged entries whose holders take under a quarter of the area, then frames over
/// three laps: each frame gets room within 200 commit spans, a trim passes the
/// first frame, and `newest` gives each tagged entry.
async fn live(shard: Shard, shape: Shape) {
    let shard = trimmed(shard);
    let body_max = shape.blocks * to_usize(BLOCK) - 9;
    let mut slots = Slots::new();
    let buffer = shard
        .open(layout(LIVE_AREA, body_max), &mut slots)
        .await
        .expect("opens");
    let quarter = to_usize(LIVE_AREA) / 4;
    let count = (1..quarter)
        .take_while(|&n| carried_len(shape.idle, n, shape.len) + 42 <= quarter)
        .last()
        .unwrap_or(0);
    let mut expected = Vec::new();
    let indexes: Vec<u32> = (100..).take(count).collect();
    for commit in indexes.chunks(shape.per * shape.batches) {
        for batch in commit.chunks(shape.per) {
            let entries: Vec<_> = batch
                .iter()
                .map(|&index| {
                    let slot = slots.index(key(index));
                    expected.push((slot, kept(&shard.block(shape.len))));
                    tagged(index, slot, Path::Live, 0, 1, shard.block(shape.len))
                })
                .collect();
            append_within(&buffer, &shard.clock, &entries, 200)
                .await
                .expect("the ring frees room for the state");
        }
        buffer.committed().await.expect("commits");
    }
    let idle: Vec<_> = (10_000..)
        .take(shape.idle)
        .map(|index| {
            let slot = slots.index(key(index));
            entry(index, slot, Path::Live, 0, 1, None, Parts::default())
        })
        .collect();
    append_within(&buffer, &shard.clock, &idle, 200)
        .await
        .expect("the ring frees room for the idle tails");
    buffer.committed().await.expect("commits");
    let data = slots.index(key(9));
    frames(&buffer, &shard, data, &shape).await;
    assert!(trimmed_past_the_start(&buffer, data).await);
    for entry in &idle {
        assert_eq!(buffer.durable(entry.slot, Path::Live), tail(1, None));
    }
    expected.sort_unstable();
    let mut newest = newest(&buffer, Path::Live, 1);
    newest.sort_unstable();
    assert_eq!(newest, expected);
}

/// Frames of index 9 at `data` over three laps of the ring, in commits of
/// `shape.frames` frames, each of which gets room within 200 commit spans.
async fn frames(buffer: &Buffer, shard: &Shard, data: Slot, shape: &Shape) {
    let laps = 3 * LIVE_AREA / shape.frame.next_multiple_of(to_usize(BLOCK)) as u64;
    let mut seq = 0;
    while seq < laps {
        for _ in 0..shape.frames {
            let last = Some(i64::try_from(seq).expect("a small seq") + 1);
            let frame = entry(
                9,
                data,
                Path::Live,
                seq,
                1,
                last,
                shard.block(shape.frame).into(),
            );
            append_within(buffer, &shard.clock, &[frame], 200)
                .await
                .expect("the ring frees room for a frame");
            seq += 1;
        }
        buffer.committed().await.expect("commits");
    }
}

/// Holders of three blocks in each record, behind a carry of one record each
/// commit.
#[test]
fn lives_with_a_carry_behind_the_reach() {
    let shape = Shape {
        blocks: 3,
        len: 1087,
        per: 10,
        batches: 3,
        frame: 12_000,
        frames: 5,
        idle: 0,
    };
    run(406, move |shard| live(shard, shape));
}

/// Records of one block whose carry bodies take two.
#[test]
fn lives_with_carry_bodies_larger_than_their_records() {
    let shape = Shape {
        blocks: 3,
        len: 4000,
        per: 1,
        batches: 3,
        frame: 12_000,
        frames: 5,
        idle: 0,
    };
    run(407, move |shard| live(shard, shape));
}

/// One record of more idle tails than two blocks hold, which becomes the oldest
/// holder while the copies put the carry behind.
#[test]
fn lives_with_idle_tails_over_two_blocks_in_one_record() {
    let shape = Shape {
        blocks: 3,
        len: 64,
        per: 10,
        batches: 3,
        frame: 12_000,
        frames: 5,
        idle: 240,
    };
    run(408, move |shard| live(shard, shape));
}

/// The oldest holder needs a carry record of four blocks while the head is two
/// blocks before the end of the area, so the carry skips them.
#[test]
fn lives_with_a_carry_that_skips_to_the_start() {
    let shape = Shape {
        blocks: 4,
        len: 1765,
        per: 8,
        batches: 4,
        frame: 13_249,
        frames: 4,
        idle: 0,
    };
    run(5_519_891_853_575_910_298, move |shard| live(shard, shape));
}

/// A buffer with trims or without refuses a batch whose holders one carry record
/// does not hold: two tagged entries of 4032 bytes at the least body, and 14 at a
/// body of 60000 bytes, whose carry body is over 42970 bytes.
#[test]
fn refuses_a_batch_whose_holders_pass_a_carry_record_with_trims_or_without() {
    for trimmed in [false, true] {
        refuses_holders_over_a_carry_record(trimmed, 2, BODY_MAX, (8254, 8183));
        refuses_holders_over_a_carry_record(trimmed, 14, 60_000, (57_754, 42_970));
    }
}

/// Appends `count` tagged entries of 4032 bytes to a buffer of bodies of at most
/// `body_max` bytes, which refuses them as [`Limit::Carry`] of `carry`.
fn refuses_holders_over_a_carry_record(
    trimmed: bool,
    count: u32,
    body_max: usize,
    carry: (usize, usize),
) {
    run(409, move |shard| async move {
        let shard = Shard { trimmed, ..shard };
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(128 * BLOCK, body_max), &mut slots)
            .await
            .expect("opens");
        let batch: Vec<_> = (1..=count)
            .map(|index| {
                let slot = slots.index(key(index));
                tagged(
                    index,
                    slot,
                    Path::Live,
                    0,
                    1,
                    shard.block(Layout::ENTRY_MAX_MIN),
                )
            })
            .collect();
        let (len, max) = carry;
        assert_eq!(
            buffer.append(batch),
            Err(Rejected::Large(Limit::Carry { len, max })),
            "trimmed: {trimmed}, body_max: {body_max}"
        );
    });
}

/// Two tagged entries of 3900 bytes in one record of a run with no trims, then
/// frames over three laps after an open with trims: each frame gets room, a trim
/// passes the first frame, and `newest` gives both entries.
#[test]
fn a_ring_written_with_no_trims_opens_with_trims() {
    run(409, |shard| async move {
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let (a, b) = (slots.index(key(1)), slots.index(key(2)));
        let batch = [
            tagged(1, a, Path::Live, 0, 1, shard.block(3900)),
            tagged(2, b, Path::Live, 0, 1, shard.block(3900)),
        ];
        commit(&shard, &buffer, &batch).await;
        let shard = trimmed(shard);
        let (buffer, mut slots) = reopen(&shard, buffer).await;
        let data = slots.index(key(9));
        for seq in 0..LAPS {
            commit(&shard, &buffer, &[frame(&shard, data, seq)]).await;
        }
        assert!(trimmed_past_the_start(&buffer, data).await);
        let (a, b) = (slots.index(key(1)), slots.index(key(2)));
        let kept = kept(&shard.block(3900));
        assert_eq!(
            newest(&buffer, Path::Live, 1),
            vec![(a, kept.clone()), (b, kept)]
        );
    });
}

/// A data record whose holders one carry record of the layout does not hold, as no
/// buffer writes it, gives `Invalid` at the open, with trims or without: a record of
/// a tagged entry of 4032 bytes and one of 4000 whose tag is made 1.
#[test]
fn the_open_refuses_a_record_whose_holders_pass_a_carry_record() {
    run(409, |shard| async move {
        let mut slots = Slots::new();
        let buffer = shard
            .open(layout(AREA, BODY_MAX), &mut slots)
            .await
            .expect("opens");
        let (a, b) = (slots.index(key(1)), slots.index(key(2)));
        let batch = [
            tagged(1, a, Path::Live, 0, 1, shard.block(Layout::ENTRY_MAX_MIN)),
            tagged(2, b, Path::Live, 0, 0, shard.block(4000)),
        ];
        commit(&shard, &buffer, &batch).await;
        let ending = buffer.ended();
        drop(buffer);
        ending.await.expect("ends");
        let tag = 4 + 51 + 46;
        shard.tamper_record(BLOCK, tag, &[1]).await;
        shard.open_invalid(layout(AREA, BODY_MAX), BLOCK).await;
        trimmed(shard)
            .open_invalid(layout(AREA, BODY_MAX), BLOCK)
            .await;
    });
}

/// The body of a carry record with `tails` and `copies` copies of tag 1 of index
/// 77, an entry with no samples at seq 0 and the bytes `old`.
fn carry_body_with_copies(tails: &[Vec<u8>], copies: u16) -> Vec<u8> {
    let counts = [
        &u16::try_from(tails.len()).expect("few tails").to_le_bytes()[..],
        &copies.to_le_bytes(),
    ];
    let header = [
        &77u128.to_le_bytes()[..],
        &[0],
        &0u64.to_le_bytes(),
        &0u32.to_le_bytes(),
        &7i64.to_le_bytes(),
        &[&[0][..], &1u64.to_le_bytes()].concat(),
        &[1],
        &3u32.to_le_bytes(),
    ];
    let copies = usize::from(copies);
    [
        counts.concat(),
        tails.concat(),
        header.concat().repeat(copies),
        b"old".repeat(copies),
    ]
    .concat()
}

/// A carry record that holds a copy of a path that no record and no tail holds
/// gives `Invalid` at the record, as no buffer writes it. With the tail of the
/// path, the same ring opens and gives the copy.
#[test]
fn the_open_refuses_a_carried_copy_of_a_path_that_no_record_holds() {
    run(411, |shard| async move {
        shard.create_two_records().await;
        let ring = layout(AREA, BODY_MAX);
        shard
            .put_carry(3 * BLOCK, &carry_body_with_copies(&[], 1))
            .await;
        shard.open_invalid(ring, 3 * BLOCK).await;
        let tail = carried(77, 0, 1, None);
        shard
            .put_carry(3 * BLOCK, &carry_body_with_copies(&[tail], 1))
            .await;
        let mut slots = Slots::new();
        let buffer = shard.open(ring, &mut slots).await.expect("opens");
        let copy = vec![(slots.index(key(77)), kept(b"old"))];
        assert_eq!(newest(&buffer, Path::Live, 1), copy);
    });
}

/// A carry record that holds the copy of tag 1 of index 77 twice gives `Invalid` at
/// the record, both when a data record before it holds an older entry of the tag
/// and when it holds a newer one. With one copy, the ring opens, and `newest` gives
/// the carried bytes when the data entry has the same mark, and the data bytes when
/// it is newer.
#[test]
fn the_open_refuses_a_carry_record_that_holds_a_copy_twice() {
    for first in [0, 4] {
        run(412, move |shard| async move {
            let ring = layout(AREA, BODY_MAX);
            let mut slots = Slots::new();
            let buffer = shard.open(ring, &mut slots).await.expect("opens");
            let slot = slots.index(key(77));
            let batch = [tagged(77, slot, Path::Live, first, 1, shard.block(10))];
            commit(&shard, &buffer, &batch).await;
            drop(buffer);
            let tails = [carried(77, first, 1, None)];
            shard
                .put_carry(2 * BLOCK, &carry_body_with_copies(&tails, 2))
                .await;
            shard.open_invalid(ring, 2 * BLOCK).await;
            shard
                .put_carry(2 * BLOCK, &carry_body_with_copies(&tails, 1))
                .await;
            let mut slots = Slots::new();
            let buffer = shard.open(ring, &mut slots).await.expect("opens");
            let bytes = if first == 0 {
                kept(b"old")
            } else {
                kept(&shard.block(10))
            };
            let expected = vec![(slots.index(key(77)), bytes)];
            let found = newest(&buffer, Path::Live, 1);
            assert_eq!(found, expected, "a data entry at {first}");
        });
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Any shape whose live holders are under a quarter of the area: the ring gives
    /// room again after `Full`, through records at the carry budget, commits of a
    /// large part of the area, and small tagged entries.
    #[test]
    fn lives_with_holders_under_a_quarter_of_the_area(
        seed in any::<u64>(),
        shape in shape(),
    ) {
        run(seed, move |shard| live(shard, shape));
    }
}
