//! The scalar value of an OPC UA variable.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::mem::{Discriminant, discriminant};

use types::sample::Scalar;

use crate::ffi::{self, Status};

/// Defines [`Value`], `SCALARS`, and the map of a value to its sample type, its
/// sample bytes, and its C scalar from one list, so no type can miss one of them. Each
/// entry is the variant, which is also the name of its sample type, its Rust type, its
/// kind in `ffi`, its field of `ffi::Scalar`, and the function of its little-endian
/// bytes.
macro_rules! values {
    ($($variant:ident($rust:ty) = $kind:ident, $field:ident, $bytes:expr;)*) => {
        /// The scalar value of an OPC UA variable, in the sample type of the same width
        /// and sign: Boolean as `bool`, `SByte` to `UInt64` as `i8` to `u64`, Float as
        /// `f32`, and Double as `f64`.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub(crate) enum Value {
            $($variant($rust),)*
        }

        /// The sample types of a [`Value`], in the order of their OPC UA types.
        pub(crate) const SCALARS: [Scalar; [$(stringify!($variant)),*].len()] =
            [$(Scalar::$variant,)*];

        impl Value {
            /// Gives the scalar of `value`, an answer of a read. Else gives its
            /// status when that is not Good (a Good with a subcode is Good),
            /// `BadNoData` when it holds no value, or `BadTypeMismatch` for an array
            /// or a type that is not one of [`Value`].
            ///
            /// # Safety
            ///
            /// `value` is a live `UA_DataValue`.
            pub(crate) unsafe fn of(
                value: *const ffi::DataValue,
            ) -> Result<Self, Status> {
                let mut scalar = ffi::Scalar { uint64: 0 };
                let mut kind = 0;
                // SAFETY: `value` is live, as the caller ensures, and the outputs are
                // places to write.
                let status = Status(unsafe {
                    ffi::shim_value_scalar(value, &raw mut scalar, &raw mut kind)
                });
                if status != Status::GOOD {
                    return Err(status);
                }
                Ok(match kind {
                    $(
                        // SAFETY: `shim_value_scalar` wrote the field of the kind.
                        ffi::$kind => Self::$variant(unsafe { scalar.$field }),
                    )*
                    _ => panic!(
                        "invariant: shim_value_scalar gives a kind of `ffi::Scalar`, \
                         not {kind}"
                    ),
                })
            }

            /// The sample type of the value.
            pub(crate) fn scalar(self) -> Scalar {
                match self {
                    $(Self::$variant(_) => Scalar::$variant,)*
                }
            }

            /// The variant of a value of `scalar`, or `None` when `scalar` is not one
            /// of [`SCALARS`].
            pub(crate) fn variant(scalar: Scalar) -> Option<Discriminant<Self>> {
                match scalar {
                    $(Scalar::$variant => {
                        Some(discriminant(&Self::$variant(<$rust>::default())))
                    })*
                    _ => None,
                }
            }

            /// Writes the little-endian bytes of the value to `sample`.
            ///
            /// # Panics
            ///
            /// If `sample` is not as wide as the sample type of the value.
            pub(crate) fn put(self, sample: &mut [u8]) {
                match self {
                    $(Self::$variant(value) => sample.copy_from_slice(&$bytes(value)),)*
                }
            }

            /// The value 1 of `scalar`, which is `true` for `bool`.
            ///
            /// # Panics
            ///
            /// If `scalar` is not one of [`SCALARS`].
            #[cfg(feature = "sim")]
            pub(crate) fn one(scalar: Scalar) -> Self {
                match scalar {
                    $(Scalar::$variant => Self::$variant(<$rust>::from(true)),)*
                    other => panic!("invariant: {other:?} is not one of `SCALARS`"),
                }
            }

            /// Gives the C scalar of the value and its kind.
            #[cfg(any(test, feature = "sim"))]
            pub(crate) fn raw(self) -> (ffi::Scalar, u16) {
                match self {
                    $(Self::$variant($field) => (ffi::Scalar { $field }, ffi::$kind),)*
                }
            }
        }
    };
}

values! {
    Bool(bool) = BOOLEAN, boolean, |value| [u8::from(value)];
    I8(i8) = SBYTE, sbyte, i8::to_le_bytes;
    U8(u8) = BYTE, byte, u8::to_le_bytes;
    I16(i16) = INT16, int16, i16::to_le_bytes;
    U16(u16) = UINT16, uint16, u16::to_le_bytes;
    I32(i32) = INT32, int32, i32::to_le_bytes;
    U32(u32) = UINT32, uint32, u32::to_le_bytes;
    I64(i64) = INT64, int64, i64::to_le_bytes;
    U64(u64) = UINT64, uint64, u64::to_le_bytes;
    F32(f32) = FLOAT, float, f32::to_le_bytes;
    F64(f64) = DOUBLE, double, f64::to_le_bytes;
}

#[cfg(test)]
mod tests;
