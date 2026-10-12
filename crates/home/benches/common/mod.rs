//! Items that the benches of `home` reuse.

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
