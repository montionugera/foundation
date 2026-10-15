//! Items that the benches of `home` reuse.

use std::sync::Arc;

use types::channel;
use types::frame::key_set::{Group, Interner, KeySet};
use types::sample::{Scalar, Sides, Type};

/// The data types that a mixed frame cycles through.
pub(crate) const MIXED: [Type; 10] = [
    Type::Scalar(Scalar::F32),
    Type::Scalar(Scalar::F64),
    Type::Scalar(Scalar::I16),
    Type::Scalar(Scalar::U8),
    Type::Scalar(Scalar::Stamp),
    Type::Array {
        element: Scalar::F32,
        len: 3,
    },
    Type::Matrix {
        element: Scalar::F32,
        sides: Sides {
            rows: 2,
            columns: 3,
        },
    },
    Type::List {
        element: Scalar::U8,
        max: 16,
    },
    Type::String,
    Type::Bytes,
];

/// The key set of one group: the index, channel 0, and `series - 1` data channels
/// whose types cycle through `types`.
pub(crate) fn set(series: usize, types: &[Type]) -> Arc<KeySet> {
    let key = |n: usize| channel::Key::from_u128(u128::try_from(n).expect("few"));
    let data: Vec<_> = (1..series)
        .map(|n| (key(n), types[(n - 1) % types.len()]))
        .collect();
    Interner::new().intern(&[Group {
        index: key(0),
        data: &data,
    }])
}
