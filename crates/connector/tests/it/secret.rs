//! The secrets that `connector::testing` writes.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Rc;
use std::task::Poll;

use connector::testing::{create_secret, create_secret_store};
use types::name::Name;
use types::time::Span;

/// Each value that reads of the secret `plant.token` give while a test writes `t1`
/// to it, and then `t2`.
fn read_during_write(seed: u64) -> Vec<Vec<u8>> {
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, |node, tasks| async move {
        let name: Name = "plant.token".parse().unwrap();
        let (files, clock) = (node.files(), node.clock());
        let store = create_secret_store(files.clone());
        let got = Rc::new(RefCell::new(Vec::new()));
        let done = Rc::new(Cell::new(false));
        let (into, read, ended) = (Rc::clone(&got), name.clone(), Rc::clone(&done));
        tasks.spawn(async move {
            loop {
                let last = ended.get();
                if let Ok(value) = store.get(&read).await {
                    into.borrow_mut().push(value.expose().to_vec());
                }
                if last {
                    break;
                }
                clock.sleep(Span::from_nanos(500)).await;
            }
        });
        create_secret(&files, &name, b"t1").await;
        create_secret(&files, &name, b"t2").await;
        done.set(true);
        node.clock().sleep(Span::SECOND).await;
        got.take()
    })
    .expect("the run ends")
}

#[test]
fn a_read_gets_no_value_or_a_whole_written_value() {
    for seed in 0..64 {
        let values: Vec<_> = (read_during_write(seed).into_iter())
            .map(|value| String::from_utf8(value).unwrap())
            .collect();
        let whole = values.iter().all(|value| value == "t1" || value == "t2");
        assert!(whole, "seed {seed}: {values:?}");
        assert_eq!(values.last().map(String::as_str), Some("t2"), "seed {seed}");
    }
}

/// The values of `plant.a` and `plant.b` after two tasks write them at once.
fn two_at_once(seed: u64) -> [Option<Vec<u8>>; 2] {
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, |node, tasks| async move {
        let files = node.files();
        let other = files.clone();
        tasks.spawn(async move {
            create_secret(&other, &"plant.a".parse().unwrap(), b"aaaa").await;
        });
        create_secret(&files, &"plant.b".parse().unwrap(), b"bb").await;
        node.clock().sleep(Span::SECOND).await;
        let store = create_secret_store(files);
        let mut got = [None, None];
        for (value, key) in got.iter_mut().zip(["plant.a", "plant.b"]) {
            let read = store.get(&key.parse().unwrap()).await;
            *value = read.ok().map(|value| value.expose().to_vec());
        }
        got
    })
    .expect("the run ends")
}

#[test]
fn two_secrets_written_at_once_both_land() {
    for seed in 0..32 {
        let got = two_at_once(seed);
        let want = [Some(b"aaaa".to_vec()), Some(b"bb".to_vec())];
        assert_eq!(got, want, "seed {seed}");
    }
}

/// The value of `plant.a` after a write of it that ends at `stop` and a write of
/// `t2`.
fn write_after_a_dropped_write(seed: u64, stop: i64) -> Option<Vec<u8>> {
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, move |node, _| async move {
        let (files, clock) = (node.files(), node.clock());
        let name: Name = "plant.a".parse().unwrap();
        {
            let mut write = pin!(create_secret(&files, &name, b"long-value"));
            let mut stop = pin!(clock.sleep(Span::from_nanos(stop)));
            poll_fn(|cx| {
                if write.as_mut().poll(cx).is_ready()
                    || stop.as_mut().poll(cx).is_ready()
                {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
        clock.sleep(Span::SECOND).await;
        create_secret(&files, &name, b"t2").await;
        let value = create_secret_store(files.clone()).get(&name).await;
        value.ok().map(|value| value.expose().to_vec())
    })
    .expect("the run ends")
}

#[test]
fn a_write_after_a_dropped_write_lands() {
    for seed in 0..32 {
        for stop in [1, 1_000, 10_000, 100_000, 1_000_000, 5_000_000] {
            let got = write_after_a_dropped_write(seed, stop);
            assert_eq!(got, Some(b"t2".to_vec()), "seed {seed} stop {stop}");
        }
    }
}
