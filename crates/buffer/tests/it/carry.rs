//! Carry records under a steady load of many paths.

use super::*;

/// `paths` live paths get one entry of one sample each in turn, four in each
/// commit, for 600 commits on a ring of 24 blocks with trims. Gives the first
/// commit that the ring still refuses after 1000 commit spans, with its error.
fn refused_with_paths(seed: u64, paths: u32) -> Option<(u64, Rejected)> {
    let (mut sim, node) = create_node(seed);
    let own = node.clone();
    let found = Arc::new(Mutex::new(None));
    let given = Arc::clone(&found);
    drop(on_node(&node, "stall", move |tasks| async move {
        let config = Config {
            layout: layout(24 * BLOCK, BODY_MAX),
            trimmed: true,
            ..node_config(&own, tasks, DIR)
        };
        let clock = own.clock();
        let mut slots = Slots::new();
        let buffer = Buffer::open(config, &mut slots).await.expect("opens");
        let mut next = 0;
        for commit in 0..600 {
            let batch: Vec<Entry> = (0..4)
                .map(|_| {
                    let index = next % paths + 1;
                    let lap = u64::from(next / paths);
                    next += 1;
                    let slot = slots.index(key(index));
                    let last = i64::try_from(lap).expect("small") + 1;
                    let parts = Parts::default();
                    entry(index, slot, Path::Live, lap, 1, Some(last), parts)
                })
                .collect();
            let mut spans = 0;
            loop {
                match buffer.append(batch.clone()) {
                    Ok(()) => break,
                    Err(Rejected::Full { .. }) if spans < 1000 => {
                        spans += 1;
                        clock.sleep(COMMIT).await;
                    }
                    Err(error) => {
                        *given.lock().expect("no panic held the lock") =
                            Some((commit, error));
                        return;
                    }
                }
            }
            buffer.committed().await.expect("commits");
        }
    }));
    sim.run_for(Span::from_nanos(600_000_000_000))
        .expect("the run goes on");
    found.lock().expect("no panic held the lock").take()
}

/// A carry record that holds one old path takes newer paths up to its limit,
/// so that 1000 paths, more than ten records of tails, never stall the ring.
#[test]
fn a_thousand_paths_with_samples_do_not_stall_the_ring() {
    let failures: Vec<_> = (0..32)
        .filter_map(|seed| refused_with_paths(seed, 1000).map(|r| (seed, r)))
        .collect();
    assert_eq!(failures, Vec::new());
}
