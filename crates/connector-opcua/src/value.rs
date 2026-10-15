//! The scalar value of an OPC UA variable.

#![expect(unsafe_code, reason = "open62541 is a C library")]

use std::fmt;

use types::sample::{self, Scalar};

use crate::ffi::{self, Status};

/// The scalar value of an OPC UA variable, in the sample type of the same width and
/// sign: Boolean as `bool`, `SByte` to `UInt64` as `i8` to `u64`, Float as `f32`, and
/// Double as `f64`.
// In the order of `sample::Value`, so that a map to it keeps the tag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Value {
    Bool(bool),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    F32(f32),
    F64(f64),
}

/// The value 1 of each type of a [`Value`], in the order of their OPC UA types.
const ONES: [Value; 11] = [
    Value::Bool(true),
    Value::I8(1),
    Value::U8(1),
    Value::I16(1),
    Value::U16(1),
    Value::I32(1),
    Value::U32(1),
    Value::I64(1),
    Value::U64(1),
    Value::F32(1.0),
    Value::F64(1.0),
];

/// The sample types of a [`Value`], in the order of their OPC UA types.
pub(crate) const SCALARS: [Scalar; 11] = {
    let mut scalars = [Scalar::Bool; 11];
    let mut at = 0;
    while at < ONES.len() {
        scalars[at] = ONES[at].sample().scalar();
        at += 1;
    }
    scalars
};

impl Value {
    /// Gives the scalar of `value`, an answer of a read. Else gives its status when
    /// that is not Good (a Good with a subcode is Good), `BadNoData` when it holds no
    /// value, or the type of an array or of a scalar that is not one of [`Value`].
    ///
    /// # Safety
    ///
    /// `value` is a live `UA_DataValue`.
    pub(crate) unsafe fn of(value: *const ffi::DataValue) -> Result<Self, Fault> {
        let mut scalar = ffi::Scalar { uint64: 0 };
        let mut kind = 0;
        let mut data_type = std::ptr::null();
        // SAFETY: `value` is live, as the caller ensures, and the outputs are places
        // to write.
        let status = Status(unsafe {
            ffi::shim_value_scalar(
                value,
                &raw mut scalar,
                &raw mut kind,
                &raw mut data_type,
            )
        });
        if status != Status::GOOD {
            return Err(Fault::Status(status));
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
            kind => return Err(mistyped(kind, data_type)),
        })
    }

    /// The sample type of the value.
    #[inline]
    pub(crate) fn scalar(self) -> Scalar {
        self.sample().scalar()
    }

    /// The sample value of the same type.
    #[inline]
    const fn sample(self) -> sample::Value {
        match self {
            Self::Bool(value) => sample::Value::Bool(value),
            Self::I8(value) => sample::Value::I8(value),
            Self::U8(value) => sample::Value::U8(value),
            Self::I16(value) => sample::Value::I16(value),
            Self::U16(value) => sample::Value::U16(value),
            Self::I32(value) => sample::Value::I32(value),
            Self::U32(value) => sample::Value::U32(value),
            Self::I64(value) => sample::Value::I64(value),
            Self::U64(value) => sample::Value::U64(value),
            Self::F32(value) => sample::Value::F32(value),
            Self::F64(value) => sample::Value::F64(value),
        }
    }

    /// The OPC UA type of the value.
    pub(crate) fn type_of(self) -> Type {
        // The kind of a scalar of `Value` is its index in `UA_TYPES`: `shim.c` asserts
        // both.
        Type {
            at: self.raw().1,
            array: false,
        }
    }

    /// The value 1 of `scalar`, which is `true` for `bool`.
    ///
    /// # Panics
    ///
    /// If `scalar` is not one of [`SCALARS`].
    #[cfg(feature = "sim")]
    pub(crate) fn one(scalar: Scalar) -> Self {
        ONES.into_iter()
            .find(|one| one.scalar() == scalar)
            .unwrap_or_else(|| panic!("invariant: {scalar:?} is not one of `SCALARS`"))
    }

    /// Gives the C scalar of the value and its kind.
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
        value.sample()
    }
}

/// The fault of a Good answer of the open62541 kind `kind` and the type `data_type`,
/// apart from the Ok path of [`Value::of`].
///
/// # Panics
///
/// When `data_type` is not of `UA_TYPES`. The client registers no type of its own.
#[cold]
#[inline(never)]
fn mistyped(kind: u16, data_type: *const ffi::DataType) -> Fault {
    let mut types = ffi::types().iter();
    let at = types
        .position(|of| std::ptr::eq(of, data_type))
        .expect("invariant: each type of a decoded value is of `UA_TYPES`");
    Fault::Type(Type {
        at: u16::try_from(at).expect("invariant: `UA_TYPES` fits a u16"),
        array: kind & ffi::ARRAY != 0,
    })
}

/// Why an answer of a read gives no [`Value`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Fault {
    /// The status of the answer, which is not Good, or `BadNoData` when it holds no
    /// value.
    Status(Status),
    /// The type of a Good answer that is an array, or a scalar of another type than
    /// those of [`Value`].
    Type(Type),
}

/// The OPC UA type of a value: a type of `UA_TYPES`, or an array of one. Its text is
/// the `typeName` of open62541, such as `String`, `Range`, or `array of Double`.
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Type {
    /// The index of the type in `UA_TYPES`.
    at: u16,
    array: bool,
}

// An index, not a reference, keeps each read of a tick at 16 bytes.
const _: () = assert!(
    size_of::<Result<Value, Fault>>() == 16,
    "a read of a tick grew"
);

impl Type {
    /// The type of the text `name`, such as `array of Double`, from `UA_TYPES`.
    ///
    /// # Panics
    ///
    /// If no type of `UA_TYPES` has the name.
    #[cfg(test)]
    pub(crate) fn named(name: &str) -> Self {
        let (array, scalar) = match name.strip_prefix("array of ") {
            Some(scalar) => (true, scalar),
            None => (false, name),
        };
        let mut types = ffi::types().iter();
        let at = types.position(|data_type| data_type.name() == scalar);
        let at = at.unwrap_or_else(|| panic!("no type {name:?}"));
        Self {
            at: u16::try_from(at).expect("a u16"),
            array,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.array {
            f.write_str("array of ")?;
        }
        f.write_str(ffi::types()[usize::from(self.at)].name())
    }
}

impl fmt::Debug for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[cfg(test)]
mod tests;
