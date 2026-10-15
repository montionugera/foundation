use types::sample::{self, Scalar};

use super::{Fault, SCALARS, Type, Value};
use crate::ffi::{self, Status};
use crate::fuzz;

/// Decodes `bytes`, the binary encoding of a `DataValue`.
fn decoded(bytes: &[u8]) -> fuzz::Value {
    let data_type = &ffi::types()[ffi::DATA_VALUE];
    let (value, read) = fuzz::Value::decode(data_type, bytes).expect("a DataValue");
    assert_eq!(read, bytes.len());
    value
}

#[test]
fn an_answer_with_a_good_status_and_no_value_fails_with_no_data() {
    assert_eq!(ffi::types()[ffi::DATA_VALUE].name(), "DataValue");
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
        Err(Fault::Type(opc_type)) => opc_type.to_string(),
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
fn a_type_equals_only_the_same_type_of_the_same_shape() {
    let [string, strings, double] =
        ["String", "array of String", "Double"].map(Type::named);
    assert_eq!(string, Type::named("String"));
    assert_ne!(string, strings);
    assert_ne!(string, double);
    assert_eq!(format!("{strings:?}"), "array of String");
}

#[test]
fn an_answer_of_a_decoded_structure_gives_the_name_of_the_structure() {
    // An ExtensionObject (22) of the binary encoding of `Range` (ns 0, id 886), with a
    // body of two Doubles.
    let range = [0x01, 0x16, 0x01, 0x00, 0x76, 0x03, 0x01, 16, 0, 0, 0];
    assert_eq!(fault(&[&range[..], &[0; 16]].concat()), "Range");
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
    let names = values.map(|value| value.type_of().to_string());
    let expected = [
        "Boolean", "SByte", "Byte", "Int16", "UInt16", "Int32", "UInt32", "Int64",
        "UInt64", "Float", "Double",
    ];
    assert_eq!(names, expected);
}

/// A value of each type, in the order of [`SCALARS`].
const VALUES: [Value; 11] = [
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

#[test]
fn the_scalars_are_the_sample_types_of_the_values_in_the_order_of_their_kinds() {
    assert_eq!(VALUES.map(Value::scalar), SCALARS);
    let kinds: Vec<u16> = VALUES.iter().map(|value| value.raw().1).collect();
    assert_eq!(kinds, (ffi::BOOLEAN..=ffi::DOUBLE).collect::<Vec<_>>());
}

#[test]
fn maps_each_value_to_the_sample_value_of_its_type() {
    let expected = [
        sample::Value::Bool(true),
        sample::Value::I8(-1),
        sample::Value::U8(1),
        sample::Value::I16(-1),
        sample::Value::U16(1),
        sample::Value::I32(-1),
        sample::Value::U32(1),
        sample::Value::I64(-1),
        sample::Value::U64(1),
        sample::Value::F32(1.0),
        sample::Value::F64(1.0),
    ];
    assert_eq!(VALUES.map(sample::Value::from), expected);
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
