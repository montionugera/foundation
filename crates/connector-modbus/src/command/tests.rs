#![expect(clippy::arithmetic_side_effects, reason = "a test may panic")]

use proptest::prelude::*;
use types::sample::{Scalar, Type, Value};

use super::*;
use crate::device::Device;
use crate::pdu::Reply;
use crate::poll::{Error, groups};

fn write(table: Table, address: u16, scalar: Scalar, order: Order) -> Write {
    Write::new(table, address, Type::Scalar(scalar), order).expect("a valid write")
}

/// Sends `request` to `device`, and asserts that the device wrote it.
fn send(device: &mut Device, request: &Request) {
    let mut pdu = Vec::new();
    request.encode(&mut pdu).expect("a valid request");
    let mut reply = Vec::new();
    device.answer(&pdu, &mut reply);
    let reply = request.decode_reply(&reply).expect("a valid reply");
    assert_eq!(reply, Reply::Written, "{request:?}");
}

/// The value that the read of `table`, `address`, and `scalar` gets from `device`.
fn read_back(
    device: &mut Device,
    table: Table,
    address: u16,
    scalar: Scalar,
    order: Order,
) -> Value {
    let read = Read::new(table, address, Type::Scalar(scalar)).expect("a valid read");
    let [group] = groups(&[read], order).try_into().expect("one group");
    let mut pdu = Vec::new();
    group.request().encode(&mut pdu).expect("a valid request");
    let mut reply = Vec::new();
    device.answer(&pdu, &mut reply);
    let reply = group.request().decode_reply(&reply).expect("a valid reply");
    let mut values = group.values(reply).expect("no exception");
    values.next().expect("one value").1
}

/// The sample bytes of `value`, so that a NaN equals itself.
fn bytes(value: Value) -> Vec<u8> {
    let mut sample = vec![0; value.scalar().width()];
    value.write(&mut sample);
    sample
}

mod request {
    use super::*;

    #[test]
    fn big_puts_the_high_word_first() {
        let f32 = write(Table::HoldingRegisters, 10, Scalar::F32, Order::Big);
        assert_eq!(
            f32.request(Value::F32(1.0)),
            Request::WriteRegisters {
                start: 10,
                values: vec![0x3F80, 0]
            }
        );
        let u64 = write(Table::HoldingRegisters, 0, Scalar::U64, Order::Big);
        assert_eq!(
            u64.request(Value::U64(0x0102_0304_0506_0708)),
            Request::WriteRegisters {
                start: 0,
                values: vec![0x0102, 0x0304, 0x0506, 0x0708]
            }
        );
    }

    #[test]
    fn little_puts_the_low_word_first() {
        let f32 = write(Table::HoldingRegisters, 10, Scalar::F32, Order::Little);
        assert_eq!(
            f32.request(Value::F32(1.0)),
            Request::WriteRegisters {
                start: 10,
                values: vec![0, 0x3F80]
            }
        );
        let i32 = write(Table::HoldingRegisters, 7, Scalar::I32, Order::Little);
        assert_eq!(
            i32.request(Value::I32(-2)),
            Request::WriteRegisters {
                start: 7,
                values: vec![0xFFFE, 0xFFFF]
            }
        );
    }

    #[test]
    fn one_register_is_function_6_in_each_order() {
        for order in [Order::Big, Order::Little] {
            let u16 = write(Table::HoldingRegisters, 10, Scalar::U16, order);
            assert_eq!(
                u16.request(Value::U16(7)),
                Request::WriteRegister {
                    address: 10,
                    value: 7
                }
            );
            let i16 = write(Table::HoldingRegisters, 3, Scalar::I16, order);
            assert_eq!(
                i16.request(Value::I16(-2)),
                Request::WriteRegister {
                    address: 3,
                    value: 0xFFFE
                }
            );
        }
    }

    #[test]
    fn a_coil_is_function_5() {
        let coil = write(Table::Coils, 4, Scalar::Bool, Order::Big);
        for value in [false, true] {
            assert_eq!(
                coil.request(Value::Bool(value)),
                Request::WriteCoil { address: 4, value }
            );
        }
    }

    #[test]
    #[should_panic(expected = "U32(7) is not a value of the write")]
    fn a_value_of_another_type_panics() {
        let u16 = write(Table::HoldingRegisters, 0, Scalar::U16, Order::Big);
        drop(u16.request(Value::U32(7)));
    }

    #[test]
    #[should_panic(expected = "U16(1) is not a value of the write")]
    fn a_number_to_a_coil_panics() {
        let coil = write(Table::Coils, 0, Scalar::Bool, Order::Big);
        drop(coil.request(Value::U16(1)));
    }
}

mod new {
    use super::*;

    #[test]
    fn read_only_tables_are_refused() {
        for (table, scalar, text) in [
            (
                Table::DiscreteInputs,
                Scalar::Bool,
                "the discrete inputs are read-only",
            ),
            (
                Table::InputRegisters,
                Scalar::U16,
                "the input registers are read-only",
            ),
            (
                Table::InputRegisters,
                Scalar::Bool,
                "the input registers are read-only",
            ),
        ] {
            let error = Write::new(table, 0, Type::Scalar(scalar), Order::Big)
                .expect_err("a read-only table");
            assert_eq!(error, Error::ReadOnly { table });
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn a_type_that_the_table_does_not_hold_is_refused() {
        let sample = Type::Scalar(Scalar::U16);
        let error = Write::new(Table::Coils, 0, sample, Order::Big)
            .expect_err("a refused type");
        assert_eq!(
            error,
            Error::Type {
                table: Table::Coils,
                sample
            }
        );
        assert_eq!(error.to_string(), "the coils hold no u16 values");
        let sample = Type::Scalar(Scalar::Bool);
        assert_eq!(
            Write::new(Table::HoldingRegisters, 0, sample, Order::Big),
            Err(Error::Type {
                table: Table::HoldingRegisters,
                sample
            })
        );
    }

    #[test]
    fn a_value_past_the_last_address_is_refused() {
        let u64 = Type::Scalar(Scalar::U64);
        let last = Write::new(Table::HoldingRegisters, 65532, u64, Order::Big)
            .expect("the last four registers");
        assert_eq!(
            last.request(Value::U64(1)),
            Request::WriteRegisters {
                start: 65532,
                values: vec![0, 0, 0, 1]
            }
        );
        let error = Write::new(Table::HoldingRegisters, 65533, u64, Order::Big)
            .expect_err("past the last address");
        assert_eq!(
            error,
            Error::Range {
                address: 65533,
                count: 4
            }
        );
        assert_eq!(
            error.to_string(),
            "a value of 4 registers from address 65533 runs past address 65535"
        );
    }
}

const SPAN: u16 = 260;

fn any_order() -> impl Strategy<Value = Order> {
    prop_oneof![Just(Order::Big), Just(Order::Little)]
}

fn any_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        any::<bool>().prop_map(Value::Bool),
        any::<u16>().prop_map(Value::U16),
        any::<i16>().prop_map(Value::I16),
        any::<u32>().prop_map(Value::U32),
        any::<i32>().prop_map(Value::I32),
        any::<f32>().prop_map(Value::F32),
        any::<u64>().prop_map(Value::U64),
        any::<i64>().prop_map(Value::I64),
        any::<f64>().prop_map(Value::F64),
    ]
}

fn any_device() -> impl Strategy<Value = Device> {
    let len = usize::from(SPAN) + 4;
    (
        prop::collection::vec(any::<bool>(), len),
        prop::collection::vec(any::<u16>(), len),
    )
        .prop_map(|(coils, holding_registers)| Device {
            coils,
            holding_registers,
            ..Device::default()
        })
}

proptest! {
    #[test]
    fn each_value_reads_back_as_written(
        value in any_value(),
        order in any_order(),
        address in 0..SPAN,
        mut device in any_device(),
    ) {
        let scalar = value.scalar();
        let table = if scalar == Scalar::Bool {
            Table::Coils
        } else {
            Table::HoldingRegisters
        };
        let before = device.clone();
        send(&mut device, &write(table, address, scalar, order).request(value));
        let got = read_back(&mut device, table, address, scalar, order);
        prop_assert_eq!(bytes(got), bytes(value));
        let start = usize::from(address);
        let end = start + scalar.width().div_ceil(2);
        let (mut was, mut is) = (before, device);
        if table == Table::Coils {
            was.coils[start] = false;
            is.coils[start] = false;
        } else {
            was.holding_registers[start..end].fill(0);
            is.holding_registers[start..end].fill(0);
        }
        prop_assert_eq!(was, is, "a write changed another item");
    }
}
