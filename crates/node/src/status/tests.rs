use std::rc::Rc;

use clock::{Clock, Status};
use env::files::{self, Operation};
use estimate::Measurement;
use proptest::prelude::*;
use sim::node::Node;
use types::digest::Digest;
use types::sample::{Scalar, Type};
use types::time::Span;

use super::{Collector, TABLE, Value};

fn host() -> (sim::Sim, Node) {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    (sim, host)
}

fn measure(host: &Node, offset: Span, error: Span) -> Measurement {
    Measurement::new(host.clock().now(), offset, error).expect("at most 36500 days")
}

fn ms(n: i64) -> Span {
    Span::from_nanos(n * 1_000_000)
}

fn data_type(value: Value) -> Type {
    match value {
        Value::U8(_) => Type::Scalar(Scalar::U8),
        Value::U64(_) => Type::Scalar(Scalar::U64),
        Value::Span(_) => Type::Scalar(Scalar::Span),
    }
}

/// Asserts that each value has the type of its channel.
fn typed(sample: [Option<Value>; TABLE.len()]) -> [Option<Value>; TABLE.len()] {
    for (channel, value) in TABLE.iter().zip(sample) {
        if let Some(value) = value {
            assert_eq!(data_type(value), channel.data_type, "{}", channel.name);
        }
    }
    sample
}

fn create_pool() -> Rc<block::Pool> {
    let config = block::Config::new(1 << 20).expect("the budget fits");
    let memory = block::Heap::new(config.reservation());
    Rc::new(block::Pool::new(config, memory))
}

/// A chunk store in the chunk directory of `host`, with no floor.
async fn create_store(host: &Node, pool: Rc<block::Pool>) -> Rc<blob::Store> {
    let config = blob::Config {
        files: host.files(),
        dir: crate::directory::blob(),
        pool,
        floor_bytes: 0,
    };
    Rc::new(blob::Store::open(config).await.expect("the store opens"))
}

/// What `body` gives for a host, its clock, and the collector of that clock and of a
/// new chunk store. No time passes in `body`.
fn run<T: Send + 'static>(
    body: impl FnOnce(&Node, &mut Clock, &Collector) -> T + Send + 'static,
) -> T {
    let (mut sim, host) = host();
    let ran = sim.run_on(&host, |host, _| async move {
        let store = create_store(&host, create_pool()).await;
        let (mut clock, reader) = Clock::new(host.clock());
        body(&host, &mut clock, &Collector::new(reader, store))
    });
    ran.expect("the run ends")
}

#[test]
fn lists_the_channels() {
    let names: Vec<&str> = TABLE.iter().map(|c| c.name).collect();
    let expected = [
        "clock.status",
        "clock.offset",
        "clock.error",
        "blob.corruptions",
        "blob.failures",
    ];
    assert_eq!(names, expected);
}

#[test]
fn an_unsynced_clock_gives_only_its_status() {
    let [status, offset, error, ..] = run(|_, _, collector| typed(collector.collect()));
    assert_eq!([status, offset, error], [Some(Value::U8(0)), None, None]);
}

#[test]
fn a_synced_clock_gives_its_offset_and_error() {
    let [status, offset, error, ..] = run(|host, clock, collector| {
        let source = clock.add();
        clock.push(source, measure(host, Span::HOUR, ms(2)));
        typed(collector.collect())
    });
    assert_eq!(
        [status, offset, error],
        [
            Some(Value::U8(1)),
            Some(Value::Span(Span::HOUR)),
            Some(Value::Span(ms(2))),
        ]
    );
}

#[test]
fn a_clock_in_holdover_keeps_its_offset() {
    let [status, offset, error, ..] = run(|host, clock, collector| {
        let source = clock.add();
        clock.push(source, measure(host, Span::HOUR, ms(2)));
        // One of two sources has no measurement, so no majority agrees.
        clock.add();
        typed(collector.collect())
    });
    assert_eq!(
        [status, offset, error],
        [
            Some(Value::U8(2)),
            Some(Value::Span(Span::HOUR)),
            Some(Value::Span(ms(2))),
        ]
    );
}

#[test]
fn the_error_of_a_clock_in_holdover_grows_by_drift() {
    let (mut sim, host) = host();
    let (mut clock, reader) = Clock::new(host.clock());
    let source = clock.add();
    clock.push(source, measure(&host, Span::HOUR, ms(2)));
    clock.add();
    assert_eq!(sim.run_for(Span::HOUR), Ok(()));
    let ran = sim.run_on(&host, |host, _| async move {
        let store = create_store(&host, create_pool()).await;
        let collector = Collector::new(reader.clone(), store);
        (reader.status(), collector.collect())
    });
    let (status, sample) = ran.expect("the run ends");
    let Status::Holdover(m, _) = status else {
        panic!("a clock with no majority holds over");
    };
    assert!(m.error() > ms(2), "{:?}", m.error());
    let [_, _, error, ..] = typed(sample);
    assert_eq!(error, Some(Value::Span(m.error())));
}

/// The two counts of the chunk store that `collector` gives.
fn counts(collector: &Collector) -> [Option<Value>; 2] {
    let [.., corruptions, failures] = typed(collector.collect());
    [corruptions, failures]
}

#[test]
fn a_get_whose_read_fails_gives_one_failure_of_the_chunk_store() {
    let (mut sim, host) = host();
    let ran = sim.run_on(&host, |host, _| async move {
        let pool = create_pool();
        let store = create_store(&host, Rc::clone(&pool)).await;
        let (_clock, reader) = Clock::new(host.clock());
        let collector = Collector::new(reader, Rc::clone(&store));
        let zero = Some(Value::U64(0));
        assert_eq!(counts(&collector), [zero, zero]);
        let mut chunk = pool.alloc(5).expect("a block");
        chunk.copy_from_slice(b"chunk");
        let chunk = chunk.freeze();
        let digest = Digest::of(&chunk);
        store.put(digest, &chunk).await.expect("the put stores");
        let path = crate::directory::blob().join(digest.to_string());
        host.fail_file(&path, Operation::ReadAt);
        let failed = blob::Error::Files(files::Error::Io {
            path,
            operation: Operation::ReadAt,
            code: 5,
        });
        assert_eq!(store.get(digest).await.unwrap_err(), failed);
        assert_eq!(counts(&collector), [zero, Some(Value::U64(1))]);
    });
    assert_eq!(ran, Ok(()));
}

#[test]
fn a_get_of_a_chunk_that_fails_its_check_gives_one_corruption() {
    let (mut sim, host) = host();
    let ran = sim.run_on(&host, |host, _| async move {
        let digest = Digest::of(b"chunk");
        let dir = crate::directory::blob();
        let files = host.files();
        files.create_dir(&dir).await.expect("makes the directory");
        let empty = files::Mode::Create { len: 0 };
        let file = files.open(&dir.join(digest.to_string()), empty).await;
        file.expect("makes the file").close().await;
        files.sync_dir(&dir).await.expect("syncs");
        let store = create_store(&host, create_pool()).await;
        let (_clock, reader) = Clock::new(host.clock());
        let collector = Collector::new(reader, Rc::clone(&store));
        assert!(store.get(digest).await.expect("the get reads").is_none());
        assert_eq!(
            counts(&collector),
            [Some(Value::U64(1)), Some(Value::U64(0))]
        );
    });
    assert_eq!(ran, Ok(()));
}

proptest! {
    #[test]
    fn gives_the_offset_and_error_that_the_clock_holds(
        offset in any::<i64>(),
        error in 0..=36_500 * Span::DAY.nanos(),
    ) {
        let (m, [_, offset, error, ..]) = run(move |host, clock, collector| {
            let source = clock.add();
            let m = measure(host, Span::from_nanos(offset), Span::from_nanos(error));
            clock.push(source, m);
            (m, typed(collector.collect()))
        });
        prop_assert_eq!(offset, Some(Value::Span(m.offset())));
        prop_assert_eq!(error, Some(Value::Span(m.error())));
    }
}
