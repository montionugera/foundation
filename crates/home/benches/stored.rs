//! The time to make the stored entry of an index frame and drop it, to read each
//! series of its body, and to make the frame of a read entry again, for frames of 16,
//! 1000, and 100,000 series of one sample: each an `f32`, or a mix of scalars, an
//! array, a matrix, a list, a string, and bytes.

mod common;

use std::fmt;
use std::sync::Arc;

use common::MIXED;
use divan::Bencher;
use divan::counter::ItemsCount;
use types::frame::key_set::{Interner, KeySet};
use types::frame::{Draft, Form, Frame, Path};
use types::sample::{Scalar, Type};
use types::time::Stamp;

fn main() {
    divan::main();
}

/// The pool of each bench.
const POOL: block::Config = match block::Config::new(1 << 24) {
    Ok(config) => config,
    Err(_) => panic!("the budget fits"),
};

/// Bytes of one sample of a type that has no width.
const VARIABLE: usize = 16;

/// A key set of one group: an index, and data series whose types cycle through the
/// types given to [`Case::new`].
struct Case {
    name: &'static str,
    series: usize,
    types: &'static [Type],
    set: Arc<KeySet>,
}

impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.series, self.name)
    }
}

impl Case {
    fn new(name: &'static str, series: usize, types: &'static [Type]) -> Self {
        let set = common::set(series, types);
        Self {
            name,
            series,
            types,
            set,
        }
    }

    /// An encoded index frame with one sample of each series.
    fn frame(&self, pool: &block::Pool) -> Frame {
        let lens: Vec<_> = (0..self.series)
            .map(|entry| {
                let data_type = self.set.entries()[entry].data_type;
                (entry, data_type.width().unwrap_or(VARIABLE))
            })
            .collect();
        let mut draft =
            Draft::new(pool, &self.set, Form::Encoded, &lens).expect("room");
        draft.set_count(0, 1);
        draft.set_seq(0, 1);
        draft.freeze(Path::Live)
    }
}

fn store(pool: &block::Pool, frame: &Frame, set: &KeySet) -> buffer::Entry {
    let (last, stored_at) = (Stamp::from_nanos(7), Stamp::from_nanos(9));
    home::bench::entry(pool, frame, set, last, stored_at).expect("room")
}

fn cases() -> Vec<Case> {
    [16, 1000, 100_000]
        .into_iter()
        .flat_map(|series| {
            [
                Case::new("f32", series, &[Type::Scalar(Scalar::F32)]),
                Case::new("mixed", series, &MIXED),
            ]
        })
        .collect()
}

#[divan::bench(args = cases(), sample_count = 1000)]
fn entry(bencher: Bencher<'_, '_>, case: &Case) {
    let pool = block::Pool::heap(POOL);
    let frame = case.frame(&pool);
    bencher
        .counter(ItemsCount::new(case.series))
        .bench_local(|| drop(divan::black_box(store(&pool, &frame, &case.set))));
}

#[divan::bench(args = cases(), sample_count = 1000)]
fn read(bencher: Bencher<'_, '_>, case: &Case) {
    let pool = block::Pool::heap(POOL);
    let frame = case.frame(&pool);
    let parts = store(&pool, &frame, &case.set).parts;
    let body: Vec<u8> = parts.into_iter().flat_map(|part| part.to_vec()).collect();
    let read: Vec<_> = home::bench::read(&body).collect();
    let entries = case.set.entries();
    let stored: Vec<_> = frame
        .iter()
        .map(|(entry, bytes)| (entries[entry].key, entries[entry].data_type, bytes))
        .collect();
    assert_eq!(
        stored.len(),
        case.series,
        "{case}: the frame lacks a series"
    );
    assert_eq!(read, stored, "{case}: the body does not hold each series");
    bencher
        .counter(ItemsCount::new(case.series))
        .bench_local(|| {
            for series in home::bench::read(divan::black_box(&body)) {
                divan::black_box(series);
            }
        });
}

#[divan::bench(args = cases(), sample_count = 1000)]
fn frame(bencher: Bencher<'_, '_>, case: &Case) {
    let pool = block::Pool::heap(POOL);
    let frame = case.frame(&pool);
    let stored = home::bench::stored(&pool, &frame, &case.set).expect("room");
    let mut decoder = home::bench::Decoder::default();
    let made = decoder
        .frame(&pool, &case.set, &stored)
        .expect("room")
        .expect("a data entry");
    assert_eq!(
        made.body()[..],
        frame.body()[..],
        "{case}: the body is not the same"
    );
    bencher
        .counter(ItemsCount::new(case.series))
        .bench_local(|| {
            let made = decoder.frame(&pool, &case.set, divan::black_box(&stored));
            drop(divan::black_box(made));
        });
}

/// As `frame`, with each frame made of the other of two key sets of one interner, as
/// when two sessions on one shard read from disk in turn. The second set has one more
/// data channel, which the frame does not hold.
#[divan::bench(args = cases(), sample_count = 1000)]
fn frame_alternating(bencher: Bencher<'_, '_>, case: &Case) {
    let pool = block::Pool::heap(POOL);
    let frame = case.frame(&pool);
    let stored = home::bench::stored(&pool, &frame, &case.set).expect("room");
    let mut interner = Interner::new();
    let one = common::intern(&mut interner, case.series, case.types);
    let other = common::intern(&mut interner, case.series + 1, case.types);
    let sets = [&one, &other];
    let mut decoder = home::bench::Decoder::default();
    let mut turn = 0;
    bencher
        .counter(ItemsCount::new(case.series))
        .bench_local(|| {
            turn ^= 1;
            let made = decoder.frame(&pool, sets[turn], divan::black_box(&stored));
            drop(divan::black_box(made));
        });
}
