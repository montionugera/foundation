//! The secrets that `connector::testing` writes.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
