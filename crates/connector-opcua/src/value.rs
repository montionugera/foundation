//! The scalar value of an OPC UA variable.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use types::sample::Scalar;

use crate::ffi::{self, Status};

/// The sample types of a [`Value`], in the order of their OPC UA types.
pub(crate) const SCALARS: [Scalar; 11] = [
    Scalar::Bool,
    Scalar::I8,
    Scalar::U8,
    Scalar::I16,
    Scalar::U16,
    Scalar::I32,
    Scalar::U32,
    Scalar::I64,
    Scalar::U64,
    Scalar::F32,
    Scalar::F64,
];

/// The scalar value of an OPC UA variable, in the sample type of the same width and
/// sign: Boolean as `bool`, `SByte` to `UInt64` as `i8` to `u64`, Float as `f32`, and
/// Double as `f64`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Value {
    Bool(bool),
    I8(i8),
    U8(u8),
    I16(i16),
    U16(u16),
    I32(i32),
    U32(u32),
    I64(i64),
    U64(u64),
    F32(f32),
    F64(f64),
}

impl Value {
    /// Gives the scalar of `value`, an answer of a read. Else gives its status when
    /// that is not Good (a Good with a subcode is Good), `BadNoData` when it holds no
    /// value, or `BadTypeMismatch` for an array or a type that is not one of [`Value`].
    ///
    /// # Safety
    ///
    /// `value` is a live `UA_DataValue`.
    pub(crate) unsafe fn of(value: *const ffi::DataValue) -> Result<Self, Status> {
        let mut scalar = ffi::Scalar { uint64: 0 };
        let mut kind = 0;
        // SAFETY: `value` is live, as the caller ensures, and the outputs are places to
        // write.
        let status = Status(unsafe {
            ffi::shim_value_scalar(value, &raw mut scalar, &raw mut kind)
        });
        if status != Status::GOOD {
            return Err(status);
        }
        Ok(match kind {
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::BOOLEAN => Self::Bool(unsafe { scalar.boolean }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::SBYTE => Self::I8(unsafe { scalar.sbyte }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::BYTE => Self::U8(unsafe { scalar.byte }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::INT16 => Self::I16(unsafe { scalar.int16 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::UINT16 => Self::U16(unsafe { scalar.uint16 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::INT32 => Self::I32(unsafe { scalar.int32 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::UINT32 => Self::U32(unsafe { scalar.uint32 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::INT64 => Self::I64(unsafe { scalar.int64 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::UINT64 => Self::U64(unsafe { scalar.uint64 }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::FLOAT => Self::F32(unsafe { scalar.float }),
            // SAFETY: `shim_value_scalar` wrote the field of the kind.
            ffi::DOUBLE => Self::F64(unsafe { scalar.double }),
            _ => panic!(
                "invariant: shim_value_scalar gives a kind of `ffi::Scalar`, not {kind}"
            ),
        })
    }

    /// The sample type of the value.
    pub(crate) fn scalar(self) -> Scalar {
        match self {
            Self::Bool(_) => Scalar::Bool,
            Self::I8(_) => Scalar::I8,
            Self::U8(_) => Scalar::U8,
            Self::I16(_) => Scalar::I16,
            Self::U16(_) => Scalar::U16,
            Self::I32(_) => Scalar::I32,
            Self::U32(_) => Scalar::U32,
            Self::I64(_) => Scalar::I64,
            Self::U64(_) => Scalar::U64,
            Self::F32(_) => Scalar::F32,
            Self::F64(_) => Scalar::F64,
        }
    }

    /// Writes the little-endian bytes of the value to `sample`.
    ///
    /// # Panics
    ///
    /// If `sample` is not as wide as the sample type of the value.
    pub(crate) fn put(self, sample: &mut [u8]) {
        match self {
            Self::Bool(value) => sample.copy_from_slice(&[u8::from(value)]),
            Self::I8(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::U8(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::I16(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::U16(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::I32(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::U32(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::I64(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::U64(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::F32(value) => sample.copy_from_slice(&value.to_le_bytes()),
            Self::F64(value) => sample.copy_from_slice(&value.to_le_bytes()),
        }
    }

    /// Gives the C scalar of the value and its kind.
    #[cfg(any(test, feature = "sim"))]
    pub(crate) fn raw(self) -> (ffi::Scalar, u16) {
        match self {
            Self::Bool(boolean) => (ffi::Scalar { boolean }, ffi::BOOLEAN),
            Self::I8(sbyte) => (ffi::Scalar { sbyte }, ffi::SBYTE),
            Self::U8(byte) => (ffi::Scalar { byte }, ffi::BYTE),
            Self::I16(int16) => (ffi::Scalar { int16 }, ffi::INT16),
            Self::U16(uint16) => (ffi::Scalar { uint16 }, ffi::UINT16),
            Self::I32(int32) => (ffi::Scalar { int32 }, ffi::INT32),
            Self::U32(uint32) => (ffi::Scalar { uint32 }, ffi::UINT32),
            Self::I64(int64) => (ffi::Scalar { int64 }, ffi::INT64),
            Self::U64(uint64) => (ffi::Scalar { uint64 }, ffi::UINT64),
            Self::F32(float) => (ffi::Scalar { float }, ffi::FLOAT),
            Self::F64(double) => (ffi::Scalar { double }, ffi::DOUBLE),
        }
    }
}

#[cfg(test)]
mod tests;
