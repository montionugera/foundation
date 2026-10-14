//! The secrets that `connector::testing` writes.

use std::cell::{Cell, RefCell};
use std::future::poll_fn;
use std::pin::pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use connector::cancel;
use connector::kind::{Channels, Context, Error, Kind, Table};
use connector::supervisor::Supervisor;
use connector::testing::{
    create_config, create_secret, create_secret_store, create_status,
};
use document::Document;
use document::diagnostic::Diagnostic;
use types::channel;
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

/// The values of `names` after two tasks write them at once.
fn two_at_once(seed: u64, names: [&'static str; 2]) -> [Option<Vec<u8>>; 2] {
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, move |node, tasks| async move {
        let files = node.files();
        let other = files.clone();
        tasks.spawn(async move {
            create_secret(&other, &names[0].parse().unwrap(), b"aaaa").await;
        });
        create_secret(&files, &names[1].parse().unwrap(), b"bb").await;
        node.clock().sleep(Span::SECOND).await;
        let store = create_secret_store(files);
        let mut got = [None, None];
        for (value, key) in got.iter_mut().zip(names) {
            let read = store.get(&key.parse().unwrap()).await;
            *value = read.ok().map(|value| value.expose().to_vec());
        }
        got
    })
    .expect("the run ends")
}

#[test]
fn two_secrets_written_at_once_both_land() {
    // The second pair has one `DefaultHasher` hash, `b2ce31a3c857fea4`, so a staged
    // file named by a hash of the name is one file for both.
    let pairs = [
        ["plant.a", "plant.b"],
        ["p.ac5d98e62022af56d", "p.a8f75ad3ac7923eff"],
    ];
    for (names, seed) in pairs
        .into_iter()
        .flat_map(|names| (0..32).map(move |seed| (names, seed)))
    {
        let got = two_at_once(seed, names);
        let want = [Some(b"aaaa".to_vec()), Some(b"bb".to_vec())];
        assert_eq!(got, want, "{names:?}, seed {seed}");
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

/// Reads `plant.token` until the store gives it, and records the value.
struct Watch(Arc<Mutex<Vec<Vec<u8>>>>);

impl Kind for Watch {
    type Config = ();

    fn parse(&self, _: &Document) -> Result<(), Vec<Diagnostic>> {
        Ok(())
    }

    fn check(&self, (): &()) -> Result<Channels, Vec<Diagnostic>> {
        Ok(Channels::default())
    }

    fn discover(
        &self,
        _: &cancel::Token,
    ) -> impl Future<Output = Result<Vec<Document>, Error>> {
        std::future::ready(Ok(Vec::new()))
    }

    async fn run(&self, ctx: Context<()>) -> Result<(), Error> {
        loop {
            match ctx.secret(&"plant.token".parse().unwrap()).await {
                Ok(value) => {
                    self.0.lock().unwrap().push(value.expose().to_vec());
                    return Ok(());
                }
                Err(Error::Retry(_)) => {
                    ctx.clock().sleep(Span::from_nanos(1_000)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

/// The values that a supervised connector reads of `plant.token` while a test
/// writes `t1` to it.
fn read_in_a_run(seed: u64) -> Vec<Vec<u8>> {
    let got = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&got);
    let mut sim = sim::Sim::new(sim::Config {
        seed,
        ..sim::Config::default()
    });
    let node = sim.node(sim::node::Config::default());
    let result = sim.run_on(&node, move |node, tasks| async move {
        let env = hub::testing::Env {
            files: node.files(),
            clock: node.clock(),
            wall: node.wall(),
            entropy: node.entropy(),
            tasks: tasks.clone(),
        };
        let kinds = Table::new().with("watch", Watch(into));
        let config = create_config(env, node.net(), kinds).await;
        let connector: Name = "plant.watch".parse().unwrap();
        let status = create_status(&connector, &[], channel::Key::from_u128(1 << 20));
        config
            .hub
            .set_definitions(status.iter().map(|(name, def)| (name, def)));
        let (files, clock) = (node.files(), node.clock());
        tasks.spawn(async move {
            clock.sleep(Span::from_nanos(50_000)).await;
            create_secret(&files, &"plant.token".parse().unwrap(), b"t1").await;
        });
        let (token, document) = (cancel::Token::new(), Document::default());
        let supervisor = Supervisor::new(config);
        let opened = supervisor.open("watch", connector, &document, &token);
        opened.await.run().await
    });
    result
        .expect("the run ends")
        .expect("the run ends with the value");
    got.lock().unwrap().clone()
}

#[test]
fn a_supervised_connector_reads_only_the_written_secret() {
    let torn: Vec<_> = (0..64)
        .map(|seed| (seed, read_in_a_run(seed)))
        .filter(|(_, got)| got != &[b"t1".to_vec()])
        .collect();
    assert_eq!(torn, [], "{} of 64 seeds read another value", torn.len());
}
