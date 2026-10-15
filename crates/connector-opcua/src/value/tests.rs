use types::sample::{self, Scalar};

use super::{SCALARS, Value};
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
    assert_eq!(reads, [Err(Status::BAD_NO_DATA); 2]);
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
