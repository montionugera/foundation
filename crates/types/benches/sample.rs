//! The time of `sample::Value::read` for each scalar, over the 1,000 samples of one
//! series in order. A time of 1 µs is 1 ns per sample.

fn main() {
    divan::main();
}

mod read {
    use std::hint::black_box;

    use divan::Bencher;
    use types::sample::{Scalar, Value};

    const SAMPLES: usize = 1000;

    /// Makes one bench for each scalar, with the scalar in the body of its own closure.
    /// One closure that captured the scalar would `match` on it in each read.
    macro_rules! reads {
        ($($name:ident = $scalar:ident,)*) => {
            // A scalar with no bench fails this match.
            const _: fn(Scalar) = |scalar| match scalar {
                $(Scalar::$scalar)|* => {}
            };
            $(
                #[divan::bench]
                fn $name(bencher: Bencher<'_, '_>) {
                    const SCALAR: Scalar = Scalar::$scalar;
                    let series = vec![1_u8; SAMPLES * SCALAR.width()];
                    bencher.bench_local(|| {
                        for sample in black_box(&series).chunks_exact(SCALAR.width()) {
                            black_box(Value::read(SCALAR, sample));
                        }
                    });
                }
            )*
        };
    }

    reads! {
        bool = Bool,
        i8 = I8,
        i16 = I16,
        i32 = I32,
        i64 = I64,
        u8 = U8,
        u16 = U16,
        u32 = U32,
        u64 = U64,
        f32 = F32,
        f64 = F64,
        stamp = Stamp,
        span = Span,
        uuid = Uuid,
    }
}
