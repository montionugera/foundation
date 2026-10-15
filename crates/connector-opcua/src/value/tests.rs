use std::mem::discriminant;

use proptest::prelude::*;
use types::sample::Scalar;

use super::{BuiltIn, Fault, SCALARS, Value};
use crate::ffi::{self, Status};
use crate::fuzz;

/// The index of `DataValue` in `UA_TYPES`.
const DATA_VALUE: usize = 22;

/// Decodes `bytes`, the binary encoding of a `DataValue`.
fn decoded(bytes: &[u8]) -> fuzz::Value {
    let data_type = &ffi::types()[DATA_VALUE];
    let (value, read) = fuzz::Value::decode(data_type, bytes).expect("a DataValue");
    assert_eq!(read, bytes.len());
    value
}

#[test]
fn an_answer_with_a_good_status_and_no_value_fails_with_no_data() {
    assert_eq!(ffi::types()[DATA_VALUE].name(), "DataValue");
    // The mask 0x00 encodes no field, and 0x02 encodes only the status, GoodClamped.
    let answers = [decoded(&[0x00]), decoded(&[0x02, 0x00, 0x00, 0x30, 0x00])];
    // SAFETY: each answer is a decoded `UA_DataValue` that lives to the end.
    let reads = answers
        .each_ref()
        .map(|answer| unsafe { Value::of(answer.pointer().cast()) });
    assert_eq!(reads, [Err(Fault::Status(Status::BAD_NO_DATA)); 2]);
}

/// The text of the fault of the answer `bytes`, an encoded `DataValue`.
fn fault(bytes: &[u8]) -> String {
    let answer = decoded(bytes);
    // SAFETY: the answer is a decoded `UA_DataValue` that lives to the end.
    match unsafe { Value::of(answer.pointer().cast()) } {
        Err(Fault::Type(built_in)) => built_in.to_string(),
        other => panic!("not a value of another type: {other:?}"),
    }
}

#[test]
fn an_answer_of_another_type_gives_its_opc_ua_type() {
    // The mask 0x01 encodes only the value. A Variant starts with its type id (1 for
    // Boolean), plus 0x80 for an array, which a count of Int32 follows.
    assert_eq!(fault(&[0x01, 0x0C, 0x02, 0, 0, 0, b'a', b'b']), "String");
    assert_eq!(
        fault(&[0x01, 0x8B, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        "array of Double"
    );
    assert_eq!(fault(&[0x01, 0x8C, 0x00, 0, 0, 0]), "array of String");
    assert_eq!(fault(&[&[0x01, 0x0E][..], &[0; 16]].concat()), "Guid");
}

#[test]
fn an_answer_with_an_empty_value_fails_with_no_data() {
    let answer = decoded(&[0x01, 0x00]);
    // SAFETY: the answer is a decoded `UA_DataValue` that lives to the end.
    let read = unsafe { Value::of(answer.pointer().cast()) };
    assert_eq!(read, Err(Fault::Status(Status::BAD_NO_DATA)));
}

#[test]
fn an_answer_with_the_status_bad_type_mismatch_gives_that_status() {
    // The mask 0x03 encodes a Double 0, then the status 0x80740000.
    let mut bytes = vec![0x03, 0x0B];
    bytes.extend([0; 8]);
    bytes.extend(0x8074_0000_u32.to_le_bytes());
    let answer = decoded(&bytes);
    // SAFETY: the answer is a decoded `UA_DataValue` that lives to the end.
    let read = unsafe { Value::of(answer.pointer().cast()) };
    assert_eq!(read, Err(Fault::Status(Status::BAD_TYPE_MISMATCH)));
}

#[test]
fn each_value_gives_the_opc_ua_name_of_its_type() {
    let values = [
        Value::Bool(false),
        Value::I8(0),
        Value::U8(0),
        Value::I16(0),
        Value::U16(0),
        Value::I32(0),
        Value::U32(0),
        Value::I64(0),
        Value::U64(0),
        Value::F32(0.0),
        Value::F64(0.0),
    ];
    let names = values.map(|value| value.built_in().to_string());
    let expected = [
        "Boolean", "SByte", "Byte", "Int16", "UInt16", "Int32", "UInt32", "Int64",
        "UInt64", "Float", "Double",
    ];
    assert_eq!(names, expected);
}

#[test]
#[should_panic(expected = "invariant: no open62541 kind is 31")]
fn a_kind_past_the_built_in_types_panics() {
    let _: BuiltIn = BuiltIn::new(31);
}

/// A value of any type.
fn any_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<i8>().prop_map(Value::I8),
        any::<u8>().prop_map(Value::U8),
        any::<i16>().prop_map(Value::I16),
        any::<u16>().prop_map(Value::U16),
        any::<i32>().prop_map(Value::I32),
        any::<u32>().prop_map(Value::U32),
        any::<i64>().prop_map(Value::I64),
        any::<u64>().prop_map(Value::U64),
        any::<f32>().prop_map(Value::F32),
        any::<f64>().prop_map(Value::F64),
    ]
}

/// The little-endian bytes of `value`.
fn bytes(value: Value) -> Vec<u8> {
    match value {
        Value::Bool(value) => vec![u8::from(value)],
        Value::I8(value) => value.to_le_bytes().to_vec(),
        Value::U8(value) => value.to_le_bytes().to_vec(),
        Value::I16(value) => value.to_le_bytes().to_vec(),
        Value::U16(value) => value.to_le_bytes().to_vec(),
        Value::I32(value) => value.to_le_bytes().to_vec(),
        Value::U32(value) => value.to_le_bytes().to_vec(),
        Value::I64(value) => value.to_le_bytes().to_vec(),
        Value::U64(value) => value.to_le_bytes().to_vec(),
        Value::F32(value) => value.to_le_bytes().to_vec(),
        Value::F64(value) => value.to_le_bytes().to_vec(),
    }
}

proptest! {
    #[test]
    fn put_writes_the_little_endian_bytes_of_the_value(value in any_value()) {
        let mut sample = vec![0xAA; value.scalar().width()];
        value.put(&mut sample);
        prop_assert_eq!(sample, bytes(value));
    }

    #[test]
    fn put_panics_for_a_sample_of_another_width(
        value in any_value(),
        more in 1..8_usize,
    ) {
        let width = value.scalar().width();
        for len in [width - 1, width + more] {
            let put = std::panic::catch_unwind(|| value.put(&mut vec![0; len]));
            let payload = put.expect_err("a put of another width panics");
            let message = payload.downcast_ref::<String>().map(String::as_str);
            let expected = format!(
                "copy_from_slice: source slice length ({width}) does not match \
                 destination slice length ({len})"
            );
            prop_assert_eq!(message, Some(expected.as_str()), "{:?}", value);
        }
    }
}

#[test]
fn the_scalars_are_the_sample_types_of_the_values_in_the_order_of_their_kinds() {
    let values = [
        Value::Bool(true),
        Value::I8(-1),
        Value::U8(1),
        Value::I16(-1),
        Value::U16(1),
        Value::I32(-1),
        Value::U32(1),
        Value::I64(-1),
        Value::U64(1),
        Value::F32(1.0),
        Value::F64(1.0),
    ];
    assert_eq!(values.map(Value::scalar), SCALARS);
    let kinds: Vec<u16> = values.iter().map(|value| value.raw().1).collect();
    assert_eq!(kinds, (ffi::BOOLEAN..=ffi::DOUBLE).collect::<Vec<_>>());
}

#[test]
fn the_variant_of_each_scalar_is_that_of_its_values_and_none_for_another_type() {
    for scalar in SCALARS {
        let variant = Value::variant(scalar).expect("one of `SCALARS`");
        let values = [true, false].map(|any| match scalar {
            Scalar::Bool => Value::Bool(any),
            Scalar::I8 => Value::I8(i8::from(any)),
            Scalar::U8 => Value::U8(u8::from(any)),
            Scalar::I16 => Value::I16(i16::from(any)),
            Scalar::U16 => Value::U16(u16::from(any)),
            Scalar::I32 => Value::I32(i32::from(any)),
            Scalar::U32 => Value::U32(u32::from(any)),
            Scalar::I64 => Value::I64(i64::from(any)),
            Scalar::U64 => Value::U64(u64::from(any)),
            Scalar::F32 => Value::F32(f32::from(any)),
            Scalar::F64 => Value::F64(f64::from(any)),
            other => panic!("{other:?} is not one of `SCALARS`"),
        });
        for value in values {
            assert_eq!(discriminant(&value), variant, "{value:?}");
        }
    }
    for scalar in [Scalar::Stamp, Scalar::Span, Scalar::Uuid] {
        assert_eq!(Value::variant(scalar), None, "{scalar:?}");
    }
}

#[cfg(feature = "sim")]
#[test]
fn one_is_1_of_each_scalar_and_panics_for_another_type() {
    let ones = SCALARS.map(Value::one);
    let expected = [
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
    assert_eq!(ones, expected);
    let panic = std::panic::catch_unwind(|| Value::one(Scalar::Stamp));
    let payload = panic.expect_err("a stamp is not one of `SCALARS`");
    assert_eq!(
        payload.downcast_ref::<String>().map(String::as_str),
        Some("invariant: Stamp is not one of `SCALARS`")
    );
}
