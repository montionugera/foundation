use proptest::prelude::*;

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

/// The little-endian bytes of `value`, from its C scalar, whose field is at offset 0.
fn bytes(value: Value) -> Vec<u8> {
    let (scalar, _) = value.raw();
    // SAFETY: each field of the union is plain bytes, and a scalar is 8 bytes.
    let all: [u8; 8] = unsafe { std::mem::transmute(scalar) };
    let mut native = all[..value.scalar().width()].to_vec();
    if cfg!(target_endian = "big") {
        native.reverse();
    }
    native
}

proptest! {
    #[test]
    fn put_writes_the_little_endian_bytes_of_the_value(value in any_value()) {
        let mut sample = vec![0xAA; value.scalar().width()];
        value.put(&mut sample);
        prop_assert_eq!(sample, bytes(value));
    }

    #[test]
    fn put_panics_for_a_sample_of_another_width(value in any_value(), more in 1..8_usize) {
        let width = value.scalar().width();
        for len in [width - 1, width + more] {
            let put = std::panic::catch_unwind(|| value.put(&mut vec![0; len]));
            prop_assert!(put.is_err(), "{value:?} into {len} bytes");
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
