//! The scalar value of an OPC UA variable, as a [`Value`] of the sample type of the
//! same width and sign.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use types::sample::{Scalar, Value};

use crate::ffi::{self, Status};

/// The sample types of the OPC UA scalars, in the order of their kinds in `ffi`:
/// Boolean as `bool`, `SByte` to `UInt64` as `i8` to `u64`, Float as `f32`, and
/// Double as `f64`.
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

/// Gives the scalar of `value`, an answer of a read. Else gives its status when that
/// is not Good (a Good with a subcode is Good), `BadNoData` when it holds no value, or
/// `BadTypeMismatch` for an array or a type whose sample type is not one of
/// [`SCALARS`].
///
/// # Safety
///
/// `value` is a live `UA_DataValue`.
pub(crate) unsafe fn of(value: *const ffi::DataValue) -> Result<Value, Status> {
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
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::BOOLEAN => Value::Bool(unsafe { scalar.boolean }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::SBYTE => Value::I8(unsafe { scalar.sbyte }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::BYTE => Value::U8(unsafe { scalar.byte }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::INT16 => Value::I16(unsafe { scalar.int16 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::UINT16 => Value::U16(unsafe { scalar.uint16 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::INT32 => Value::I32(unsafe { scalar.int32 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::UINT32 => Value::U32(unsafe { scalar.uint32 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::INT64 => Value::I64(unsafe { scalar.int64 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::UINT64 => Value::U64(unsafe { scalar.uint64 }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::FLOAT => Value::F32(unsafe { scalar.float }),
        // SAFETY: `shim_value_scalar` wrote the field of `kind`.
        ffi::DOUBLE => Value::F64(unsafe { scalar.double }),
        _ => panic!(
            "invariant: shim_value_scalar gives a kind of `ffi::Scalar`, not {kind}"
        ),
    })
}

/// The value 1 of `scalar`, which is `true` for `bool`.
///
/// # Panics
///
/// If `scalar` is not one of [`SCALARS`].
#[cfg(feature = "sim")]
pub(crate) fn one(scalar: Scalar) -> Value {
    match scalar {
        Scalar::Bool => Value::Bool(true),
        Scalar::I8 => Value::I8(1),
        Scalar::U8 => Value::U8(1),
        Scalar::I16 => Value::I16(1),
        Scalar::U16 => Value::U16(1),
        Scalar::I32 => Value::I32(1),
        Scalar::U32 => Value::U32(1),
        Scalar::I64 => Value::I64(1),
        Scalar::U64 => Value::U64(1),
        Scalar::F32 => Value::F32(1.0),
        Scalar::F64 => Value::F64(1.0),
        other => panic!("invariant: {other:?} is not one of `SCALARS`"),
    }
}

/// Gives the C scalar of `value` and its kind.
///
/// # Panics
///
/// If the sample type of `value` is not one of [`SCALARS`].
#[cfg(any(test, feature = "sim"))]
pub(crate) fn raw(value: Value) -> (ffi::Scalar, u16) {
    match value {
        Value::Bool(boolean) => (ffi::Scalar { boolean }, ffi::BOOLEAN),
        Value::I8(sbyte) => (ffi::Scalar { sbyte }, ffi::SBYTE),
        Value::U8(byte) => (ffi::Scalar { byte }, ffi::BYTE),
        Value::I16(int16) => (ffi::Scalar { int16 }, ffi::INT16),
        Value::U16(uint16) => (ffi::Scalar { uint16 }, ffi::UINT16),
        Value::I32(int32) => (ffi::Scalar { int32 }, ffi::INT32),
        Value::U32(uint32) => (ffi::Scalar { uint32 }, ffi::UINT32),
        Value::I64(int64) => (ffi::Scalar { int64 }, ffi::INT64),
        Value::U64(uint64) => (ffi::Scalar { uint64 }, ffi::UINT64),
        Value::F32(float) => (ffi::Scalar { float }, ffi::FLOAT),
        Value::F64(double) => (ffi::Scalar { double }, ffi::DOUBLE),
        other => panic!("invariant: {other:?} is not of one of `SCALARS`"),
    }
}

#[cfg(test)]
mod tests;
