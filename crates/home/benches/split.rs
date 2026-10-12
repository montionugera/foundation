//! The time to check and split a writer's frame: encode each raw series or validate
//! each encoded one, walk the stamps of the index, and make and drop the index frame.
//! Frames of one group of 16 and 1000 series of `SAMPLES` samples, raw and encoded:
//! each an `f32`, or a mix of scalars, an array, a matrix, a list, a string, and bytes.
//! Before it times a case, the bench asserts that the split walks each stamp, that the
//! index frame holds the bytes of each series encoded, and that a split with a warm
//! scratch makes no heap allocation.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

mod common;

use std::fmt;
use std::sync::Arc;

use common::MIXED;
use divan::Bencher;
use divan::counter::ItemsCount;
use home::bench::Scratch;
use types::frame::key_set::KeySet;
use types::frame::{Draft, Form};
use types::sample::{Scalar, Type};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    divan::main();
}

/// The pool of each bench.
const POOL: block::Config = match block::Config::new(1 << 26) {
    Ok(config) => config,
    Err(_) => panic!("the budget fits"),
};

/// The samples of each series.
const SAMPLES: u32 = 16;

/// The elements of one sample of a type that has no width.
const VARIABLE: usize = 16;

/// A writer's frame of one group: an index, and data series whose types cycle through
/// the types given to [`Case::new`].
struct Case {
    name: &'static str,
    series: usize,
    form: Form,
    set: Arc<KeySet>,
    /// The bytes of each series, by entry, in `form`.
    bytes: Vec<Vec<u8>>,
    /// The bytes of each series, by entry, encoded.
    encoded: Vec<Vec<u8>>,
}

impl fmt::Display for Case {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let form = match self.form {
            Form::Raw => "raw",
            Form::Encoded => "encoded",
        };
        write!(f, "{} {} {form}", self.series, self.name)
    }
}

impl Case {
    fn new(name: &'static str, series: usize, form: Form, types: &[Type]) -> Self {
        let set = common::set(series, types);
        let (bytes, encoded): (Vec<_>, Vec<_>) = set
            .entries()
            .iter()
            .enumerate()
            .map(|(entry, channel)| {
                let raw = if set.index(entry) == entry {
                    (1..=i64::from(SAMPLES))
                        .flat_map(i64::to_le_bytes)
                        .collect()
                } else {
                    values(channel.data_type)
                };
                let encoded = encode(channel.data_type, &raw);
                match form {
                    Form::Raw => (raw, encoded),
                    Form::Encoded => (encoded.clone(), encoded),
                }
            })
            .unzip();
        Self {
            name,
            series,
            form,
            set,
            bytes,
            encoded,
        }
    }

    /// A writer's frame with each series of the case.
    fn draft(&self, pool: &block::Pool) -> Draft {
        let lens: Vec<_> = self.bytes.iter().map(Vec::len).enumerate().collect();
        let mut draft = Draft::new(pool, &self.set, self.form, &lens).expect("room");
        for (entry, series) in draft.iter_mut() {
            series.copy_from_slice(&self.bytes[entry]);
        }
        draft.set_count(0, SAMPLES);
        draft
    }
}

/// The raw values of `SAMPLES` samples of `data_type`. A variable sample has
/// `VARIABLE` elements, ASCII in a `String`.
fn values(data_type: Type) -> Vec<u8> {
    let samples = to_usize(SAMPLES);
    let Some(variable) = codec::Variable::of(data_type) else {
        let len = samples * data_type.width().expect("a fixed width");
        return (0..len)
            .map(|n| u8::try_from(n % 251).expect("a byte"))
            .collect();
    };
    let element = match data_type {
        Type::List { element, .. } => element,
        _ => Scalar::U8,
    };
    let sample: Vec<u8> = (0..VARIABLE * element.width())
        .map(|n| b'a' + u8::try_from(n % 26).expect("a byte"))
        .collect();
    let samples = vec![sample; samples];
    let mut out = vec![0; variable.len(&samples).expect("small samples")];
    variable.write(&samples, &mut out);
    out
}

/// `raw`, `SAMPLES` samples of `data_type`, encoded.
fn encode(data_type: Type, raw: &[u8]) -> Vec<u8> {
    let mut out = vec![0; codec::max_len(data_type, raw.len())];
    let count = to_usize(SAMPLES);
    let len = codec::Encoder::new(data_type)
        .encode(count, raw, &mut out)
        .expect("values that fit the count");
    out.truncate(len);
    out
}

fn to_usize(n: u32) -> usize {
    usize::try_from(n).expect("a u32 fits a usize")
}

fn cases() -> Vec<Case> {
    [16, 1000]
        .into_iter()
        .flat_map(|series| {
            [Form::Raw, Form::Encoded]
                .into_iter()
                .flat_map(move |form| {
                    [
                        Case::new("f32", series, form, &[Type::Scalar(Scalar::F32)]),
                        Case::new("mixed", series, form, &MIXED),
                    ]
                })
        })
        .collect()
}

#[divan::bench(args = cases(), sample_count = 1000, sample_size = 16)]
fn split(bencher: Bencher<'_, '_>, case: &Case) {
    let pool = block::Pool::heap(POOL);
    let mut scratch = Scratch::default();
    let mut checked = 0;
    let walked = home::bench::split(
        &mut scratch,
        &pool,
        &case.set,
        case.draft(&pool),
        |frame| {
            for (entry, series) in frame.iter() {
                assert_eq!(series, case.encoded[entry], "{case}: series {entry}");
                checked += 1;
            }
        },
    );
    assert_eq!(walked, to_usize(SAMPLES), "{case}: the stamps walked");
    assert_eq!(
        checked, case.series,
        "{case}: the series of the index frame"
    );
    let draft = case.draft(&pool);
    let warm = ALLOCATOR
        .count(|| home::bench::split(&mut scratch, &pool, &case.set, draft, drop));
    assert_eq!(
        warm,
        (to_usize(SAMPLES), 0),
        "{case}: a warm split allocates"
    );
    bencher
        .counter(ItemsCount::new(case.series))
        .with_inputs(|| case.draft(&pool))
        .bench_local_values(|draft| {
            home::bench::split(&mut scratch, &pool, &case.set, draft, drop)
        });
}
