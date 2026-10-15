//! The scalar value of an OPC UA variable.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use types::sample::{self, Scalar};

use crate::ffi::{self, Status};

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

impl Value {
    /// Gives the scalar of `value`, an answer of a read. Else gives its status when
    /// that is not Good (a Good with a subcode is Good), `BadNoData` when it holds no
    /// value, or `BadTypeMismatch` for an array or a type that is not one of
    /// [`Value`].
    ///
    /// # Safety
    ///
    /// `value` is a live `UA_DataValue`.
    pub(crate) unsafe fn of(value: *const ffi::DataValue) -> Result<Self, Status> {
        let mut scalar = ffi::Scalar { uint64: 0 };
        let mut kind = 0;
        // SAFETY: `value` is live, as the caller ensures, and the outputs are places
        // to write.
        let status = Status(unsafe {
            ffi::shim_value_scalar(value, &raw mut scalar, &raw mut kind)
        });
        if status != Status::GOOD {
            return Err(status);
        }
        Ok(match kind {
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::BOOLEAN => Self::Bool(unsafe { scalar.boolean }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::SBYTE => Self::I8(unsafe { scalar.sbyte }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::BYTE => Self::U8(unsafe { scalar.byte }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::INT16 => Self::I16(unsafe { scalar.int16 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::UINT16 => Self::U16(unsafe { scalar.uint16 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::INT32 => Self::I32(unsafe { scalar.int32 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::UINT32 => Self::U32(unsafe { scalar.uint32 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::INT64 => Self::I64(unsafe { scalar.int64 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::UINT64 => Self::U64(unsafe { scalar.uint64 }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::FLOAT => Self::F32(unsafe { scalar.float }),
            // SAFETY: `shim_value_scalar` wrote the field of `kind`.
            ffi::DOUBLE => Self::F64(unsafe { scalar.double }),
            _ => panic!(
                "invariant: shim_value_scalar gives a kind of `ffi::Scalar`, not {kind}"
            ),
        })
    }

    /// The sample type of the value.
    #[inline]
    pub(crate) fn scalar(self) -> Scalar {
        sample::Value::from(self).scalar()
    }

    /// The value 1 of `scalar`, which is `true` for `bool`.
    ///
    /// # Panics
    ///
    /// If `scalar` is not one of [`SCALARS`].
    #[cfg(feature = "sim")]
    pub(crate) fn one(scalar: Scalar) -> Self {
        match scalar {
            Scalar::Bool => Self::Bool(true),
            Scalar::I8 => Self::I8(1),
            Scalar::U8 => Self::U8(1),
            Scalar::I16 => Self::I16(1),
            Scalar::U16 => Self::U16(1),
            Scalar::I32 => Self::I32(1),
            Scalar::U32 => Self::U32(1),
            Scalar::I64 => Self::I64(1),
            Scalar::U64 => Self::U64(1),
            Scalar::F32 => Self::F32(1.0),
            Scalar::F64 => Self::F64(1.0),
            _ => panic!("invariant: {scalar:?} is not one of `SCALARS`"),
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

impl From<Value> for sample::Value {
    #[inline]
    fn from(value: Value) -> Self {
        match value {
            Value::Bool(value) => Self::Bool(value),
            Value::I8(value) => Self::I8(value),
            Value::U8(value) => Self::U8(value),
            Value::I16(value) => Self::I16(value),
            Value::U16(value) => Self::U16(value),
            Value::I32(value) => Self::I32(value),
            Value::U32(value) => Self::U32(value),
            Value::I64(value) => Self::I64(value),
            Value::U64(value) => Self::U64(value),
            Value::F32(value) => Self::F32(value),
            Value::F64(value) => Self::F64(value),
        }
    }
}

#[cfg(test)]
mod tests;
