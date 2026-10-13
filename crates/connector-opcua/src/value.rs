//! The scalar value of an OPC UA variable.

#![expect(unsafe_code, reason = "open62541 is a C library")]

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

impl Value {
    /// Gives the scalar of `value`, an answer of a read. Else gives its bad status,
    /// or `BadTypeMismatch` for an array or a type that is not one of [`Value`].
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
            // SAFETY: `shim_value_scalar` wrote the field of the type 0.
            0 => Self::Bool(unsafe { scalar.boolean }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 1.
            1 => Self::I8(unsafe { scalar.sbyte }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 2.
            2 => Self::U8(unsafe { scalar.byte }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 3.
            3 => Self::I16(unsafe { scalar.int16 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 4.
            4 => Self::U16(unsafe { scalar.uint16 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 5.
            5 => Self::I32(unsafe { scalar.int32 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 6.
            6 => Self::U32(unsafe { scalar.uint32 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 7.
            7 => Self::I64(unsafe { scalar.int64 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 8.
            8 => Self::U64(unsafe { scalar.uint64 }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 9.
            9 => Self::F32(unsafe { scalar.float }),
            // SAFETY: `shim_value_scalar` wrote the field of the type 10.
            10 => Self::F64(unsafe { scalar.double }),
            _ => panic!("invariant: shim_value_scalar gives a type from 0 to 10"),
        })
    }

    /// Gives the C scalar of the value and the index of its type in `UA_TYPES`.
    #[cfg(any(test, feature = "sim"))]
    pub(crate) fn raw(self) -> (ffi::Scalar, u16) {
        match self {
            Self::Bool(boolean) => (ffi::Scalar { boolean }, 0),
            Self::I8(sbyte) => (ffi::Scalar { sbyte }, 1),
            Self::U8(byte) => (ffi::Scalar { byte }, 2),
            Self::I16(int16) => (ffi::Scalar { int16 }, 3),
            Self::U16(uint16) => (ffi::Scalar { uint16 }, 4),
            Self::I32(int32) => (ffi::Scalar { int32 }, 5),
            Self::U32(uint32) => (ffi::Scalar { uint32 }, 6),
            Self::I64(int64) => (ffi::Scalar { int64 }, 7),
            Self::U64(uint64) => (ffi::Scalar { uint64 }, 8),
            Self::F32(float) => (ffi::Scalar { float }, 9),
            Self::F64(double) => (ffi::Scalar { double }, 10),
        }
    }
}
