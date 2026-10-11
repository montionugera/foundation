//! Breaker: races at the instant a control lease ends.

use std::future::Future;
use std::rc::Rc;

use hub::home::{Outcome, Refusal};
use hub::writer;
use types::channel::Slot;
use types::time::Span;

use super::{
    AREA, BODY_MAX, LEASE, POOL, SETTLE, Test, applied, config, leased, run, write,
    write_series, written_after_cut,
};

const SEEDS: u64 = 64;

fn expired() -> Outcome {
    Outcome::Refused {
        slot: Slot::new(0),
        refusal: Refusal::Expired,
    }
}

/// As `written_after_cut`, but the write after the cut is on `channels` with the
/// samples of `series`.
fn after_cut<F>(
    seed: u64,
    before: impl FnOnce(Test) -> F + Send + 'static,
    subject: &'static str,
    channels: &'static [&'static str],
    stamp_key: u128,
    value_key: u128,
) -> Vec<Outcome>
where
    F: Future<Output = ()> + 'static,
{
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    let start = |node, tasks| async move {
        let layout = buffer::Layout::new(AREA, BODY_MAX).expect("a ring");
        let mut test = Test::new(node, tasks, layout, POOL, None).await;
        test.sync().await;
        test
    };
    let config = env::shards::Config {
        name: "shard-0".into(),
        core: None,
    };
    let started = node.shards().start(config, {
        let node = node.clone();
        move |tasks| async move { before(start(node, tasks).await).await }
    });
    let _shard = started.expect("the shard starts");
    sim.run_for(Span::from_nanos(2_000_000_000))
        .expect("the run goes on");
    sim.crash(&node, sim::Crash::Power);
    sim.run_on(&node, move |node, tasks| async move {
        let test = start(node, tasks).await;
        let mut writer = test
            .hub
            .writer(super::config(subject, channels))
            .await
            .expect("opens");
        let now = test.now();
        write_series(&mut writer, &[(stamp_key, &[now][..]), (value_key, &[2][..])])
    })
    .expect("the run after the cut ends")
}

/// A holder that writes at the instant its lease ends gets `Expired` in each order,
/// and the end of its lease is recorded.
#[test]
fn breaker_refuses_a_write_of_a_holder_at_the_instant_its_lease_ends() {
    for seed in 0..SEEDS {
        let after = written_after_cut(
            seed,
            move |test| async move {
                let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
                assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
                test.clock.sleep(LEASE).await;
                eprintln!("BREAKER seed {seed} write {:?}", test.mesh.now().monotonic);
                let now = test.now();
                assert_eq!(write(&mut a, &[now], &[2]), [expired()], "seed {seed}");
                std::future::pending::<()>().await;
            },
            "subject-b",
        );
        assert_eq!(after, [applied(1)], "seed {seed}");
    }
}

/// A leased waiter that writes at the instant the holder's lease ends takes control
/// in each order, and the end of its own lease is recorded.
#[test]
fn breaker_takes_control_for_a_waiter_that_writes_at_the_instant_a_lease_ends() {
    for seed in 0..SEEDS {
        let after = written_after_cut(
            seed,
            move |test| async move {
                let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
                let mut b = test.hub.writer(leased("subject-b")).await.expect("opens");
                assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
                test.clock.sleep(LEASE).await;
                eprintln!("BREAKER seed {seed} write {:?}", test.mesh.now().monotonic);
                let now = test.now();
                assert_eq!(write(&mut b, &[now], &[2]), [applied(1)], "seed {seed}");
                std::future::pending::<()>().await;
            },
            "subject-c",
        );
        assert_eq!(after, [applied(2)], "seed {seed}");
    }
}

/// A waiter that closes at the instant the holder's lease ends: the next waiter takes
/// control in each order, and holds it through a power cut.
#[test]
fn breaker_hands_off_past_a_waiter_that_closes_at_the_instant_a_lease_ends() {
    for seed in 0..SEEDS {
        let after = written_after_cut(
            seed,
            move |test| async move {
                let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
                let b = test.hub.writer(leased("subject-b")).await.expect("opens");
                let _c = test.writer("subject-c", &["value"]).await;
                assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
                test.clock.sleep(LEASE).await;
                eprintln!("BREAKER seed {seed} write {:?}", test.mesh.now().monotonic);
                drop(b);
                std::future::pending::<()>().await;
            },
            "subject-b",
        );
        let reserved = Outcome::Refused {
            slot: Slot::new(0),
            refusal: Refusal::Reserved,
        };
        assert_eq!(after, [reserved], "seed {seed}");
    }
}

/// A writer on two indexes renews one of them: the end of the lease on the other is
/// recorded at its own time.
#[test]
fn breaker_records_the_end_of_a_lease_on_the_second_index_of_a_writer() {
    for seed in 0..SEEDS {
        let after = after_cut(
            seed,
            move |test| async move {
                let leased = writer::Config {
                    lease: Some(LEASE),
                    ..config("subject-a", &["value", "value-b"])
                };
                let mut a = test.hub.writer(leased).await.expect("opens");
                // Keep the lease on `time` while the one on `time-b` runs out.
                for _ in 0..10 {
                    test.clock.sleep(Span::from_nanos(500_000)).await;
                    let now = test.now();
                    write(&mut a, &[now], &[1]);
                }
                std::future::pending::<()>().await;
            },
            "subject-b",
            &["value-b"],
            3,
            4,
        );
        let applied = Outcome::Applied {
            slot: Slot::new(1),
            range: types::frame::Range { seq: 0, count: 1 },
        };
        assert_eq!(after, [applied], "seed {seed}");
    }
}

/// A pause of the node across the end of a lease: the timer and the write are due at
/// once when it ends.
#[test]
fn breaker_refuses_a_write_of_a_holder_after_a_pause_past_its_lease() {
    for seed in 0..SEEDS {
        let after = written_after_cut(
            seed,
            move |test| async move {
                let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
                assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
                test.node.pause(Span::from_nanos(5_000_000));
                test.clock.sleep(Span::from_nanos(1)).await;
                eprintln!("BREAKER seed {seed} write {:?}", test.mesh.now().monotonic);
                let now = test.now();
                assert_eq!(write(&mut a, &[now], &[2]), [expired()], "seed {seed}");
                std::future::pending::<()>().await;
            },
            "subject-b",
        );
        assert_eq!(after, [applied(1)], "seed {seed}");
    }
}

/// A step of the wall clock, forward or back, inside a lease does not move its end.
#[test]
fn breaker_ends_a_lease_on_the_monotonic_clock_across_a_wall_step() {
    for step in [3_600_000_000_000_i64, -3_600_000_000_000] {
        for seed in 0..SEEDS {
            let after = written_after_cut(
                seed,
                move |test| async move {
                    let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
                    assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
                    test.clock.sleep(Span::from_nanos(500_000)).await;
                    test.node.step_wall(Span::from_nanos(step));
                    test.clock.sleep(Span::from_nanos(499_999)).await;
                    let now = test.now();
                    let before = write(&mut a, &[now], &[2]);
                    assert_eq!(before, [applied(1)], "seed {seed} step {step}");
                    test.clock.sleep(LEASE).await;
                    let now = test.now();
                    let at = write(&mut a, &[now], &[3]);
                    assert_eq!(at, [expired()], "seed {seed} step {step}");
                    std::future::pending::<()>().await;
                },
                "subject-b",
            );
            // After a step back, the restart's mesh time is behind the stored stamps:
            // the gate passed, then the order refused.
            let passed = match after.as_slice() {
                [Outcome::Refused {
                    refusal: Refusal::Order(_),
                    ..
                }] => step < 0,
                outcomes => outcomes == [applied(2)],
            };
            assert!(passed, "seed {seed} step {step}: {after:?}");
        }
    }
}

/// The hub and each session drop at the instant a lease ends: both tasks end and the
/// home is let go.
#[test]
fn breaker_ends_both_tasks_when_the_hub_drops_at_the_instant_a_lease_ends() {
    for seed in 0..SEEDS {
        run(seed, move |test| async move {
            let mut a = test.hub.writer(leased("subject-a")).await.expect("opens");
            let _b = test.hub.writer(leased("subject-b")).await.expect("opens");
            assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
            let Test {
                pool,
                clock,
                commit,
                hub,
                ended,
                ..
            } = test;
            clock.sleep(LEASE).await;
            eprintln!("BREAKER seed {seed} write {:?}", clock.now());
            drop((a, _b, hub, commit));
            clock.sleep(SETTLE).await;
            assert_eq!(ended.get(), 2, "seed {seed}: the tasks ended");
            assert_eq!(Rc::strong_count(&pool), 1, "seed {seed}: the pool is free");
        });
    }
}

/// The largest lease: the gate task sleeps to a deadline far in the future and still
/// ends when the hub drops.
#[test]
fn breaker_ends_the_gate_task_with_the_largest_lease() {
    for seed in 0..4 {
        run(seed, move |test| async move {
            let leased = writer::Config {
                lease: Some(Span::from_nanos(i64::MAX)),
                ..config("subject-a", &["value"])
            };
            let mut a = test.hub.writer(leased).await.expect("opens");
            let _b = test.writer("subject-b", &["value"]).await;
            assert_eq!(write(&mut a, &[test.now()], &[1]), [applied(0)]);
            let Test {
                pool,
                clock,
                commit,
                hub,
                ended,
                ..
            } = test;
            clock.sleep(SETTLE).await;
            drop((a, _b, hub, commit));
            clock.sleep(SETTLE).await;
            assert_eq!(ended.get(), 2, "seed {seed}: the tasks ended");
            assert_eq!(Rc::strong_count(&pool), 1, "seed {seed}: the pool is free");
        });
    }
}
