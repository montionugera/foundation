#![expect(
    clippy::arithmetic_side_effects,
    clippy::unwrap_in_result,
    reason = "a test may panic"
)]

use proptest::prelude::*;
use types::sample::{Scalar, Type, Value};

use super::*;
use crate::device::Device;
use crate::pdu::{Exception, Reply, Request, Table};

const REGISTERS: [Scalar; 8] = [
    Scalar::U16,
    Scalar::I16,
    Scalar::U32,
    Scalar::I32,
    Scalar::F32,
    Scalar::U64,
    Scalar::I64,
    Scalar::F64,
];

fn read(table: Table, address: u16, scalar: Scalar) -> Read {
    Read::new(table, address, Type::Scalar(scalar)).expect("a valid read")
}

/// The sample bytes of `value`, so that a NaN equals itself.
fn bytes(value: Value) -> (Scalar, Vec<u8>) {
    let mut sample = vec![0; value.scalar().width()];
    value.write(&mut sample);
    (value.scalar(), sample)
}

/// The registers that hold `value` in `order`, in address order.
fn registers(value: Value, order: Order) -> Vec<u16> {
    let (_, sample) = bytes(value);
    let mut words: Vec<u16> = sample
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .collect();
    if order == Order::Big {
        words.reverse();
    }
    words
}

/// The value of `scalar` in `words`, the registers in address order.
fn model(scalar: Scalar, words: &[u16], order: Order) -> Value {
    let mut words = words.to_vec();
    if order == Order::Big {
        words.reverse();
    }
    let sample: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    Value::read(scalar, &sample)
}

/// Sends each request of the groups of `reads` to `device`, and gives the value of
/// each read, or the first exception.
fn poll(
    device: &mut Device,
    reads: &[Read],
    order: Order,
) -> Result<Vec<Value>, Exception> {
    let mut values = vec![None; reads.len()];
    for group in groups(reads, order) {
        let mut pdu = Vec::new();
        group.request().encode(&mut pdu).expect("a valid request");
        let mut reply = Vec::new();
        device.answer(&pdu, &mut reply);
        let reply = group.request().decode_reply(&reply).expect("a valid reply");
        for (index, value) in group.values(reply)? {
            assert_eq!(values[index].replace(value), None, "read {index} twice");
        }
    }
    Ok(values
        .into_iter()
        .map(|value| value.expect("each read has a value"))
        .collect())
}

/// The value of one read of `scalar` from holding registers that hold `words` from
/// address 3, between filler registers.
fn check(scalar: Scalar, order: Order, words: &[u16], want: Value) {
    let mut holding_registers = vec![0xA5A5; 3];
    holding_registers.extend(words);
    holding_registers.extend([0x5A5A; 3]);
    let mut device = Device {
        holding_registers,
        ..Device::default()
    };
    let reads = [read(Table::HoldingRegisters, 3, scalar)];
    let got = poll(&mut device, &reads, order).expect("no exception");
    assert_eq!(
        bytes(got[0]),
        bytes(want),
        "{scalar:?} {order:?} {words:x?}"
    );
}

mod value {
    use super::*;

    #[test]
    fn one_register_is_the_same_in_each_order() {
        for order in [Order::Big, Order::Little] {
            check(Scalar::U16, order, &[0x1234], Value::U16(0x1234));
            check(Scalar::I16, order, &[0xFFFE], Value::I16(-2));
        }
    }

    #[test]
    fn big_puts_the_high_word_first() {
        let order = Order::Big;
        check(
            Scalar::U32,
            order,
            &[0x0102, 0x0304],
            Value::U32(0x0102_0304),
        );
        check(Scalar::I32, order, &[0xFFFF, 0xFFFE], Value::I32(-2));
        check(Scalar::F32, order, &[0x3F80, 0], Value::F32(1.0));
        let words = [0x0102, 0x0304, 0x0506, 0x0708];
        check(
            Scalar::U64,
            order,
            &words,
            Value::U64(0x0102_0304_0506_0708),
        );
        check(
            Scalar::I64,
            order,
            &[0xFFFF, 0xFFFF, 0xFFFF, 0xFFFE],
            Value::I64(-2),
        );
        check(Scalar::F64, order, &[0x3FF0, 0, 0, 0], Value::F64(1.0));
    }

    #[test]
    fn little_puts_the_low_word_first() {
        let order = Order::Little;
        check(
            Scalar::U32,
            order,
            &[0x0304, 0x0102],
            Value::U32(0x0102_0304),
        );
        check(Scalar::I32, order, &[0xFFFE, 0xFFFF], Value::I32(-2));
        check(Scalar::F32, order, &[0, 0x3F80], Value::F32(1.0));
        let words = [0x0708, 0x0506, 0x0304, 0x0102];
        check(
            Scalar::U64,
            order,
            &words,
            Value::U64(0x0102_0304_0506_0708),
        );
        check(
            Scalar::I64,
            order,
            &[0xFFFE, 0xFFFF, 0xFFFF, 0xFFFF],
            Value::I64(-2),
        );
        check(Scalar::F64, order, &[0, 0, 0, 0x3FF0], Value::F64(1.0));
    }

    #[test]
    fn the_default_order_is_big() {
        assert_eq!(Order::default(), Order::Big);
    }

    #[test]
    fn bits_read_each_address() {
        let mut device = Device {
            coils: vec![
                true, false, true, true, false, false, true, false, false, true,
            ],
            discrete_inputs: vec![false, true],
            ..Device::default()
        };
        let reads = [
            read(Table::Coils, 9, Scalar::Bool),
            read(Table::Coils, 0, Scalar::Bool),
            read(Table::DiscreteInputs, 1, Scalar::Bool),
            read(Table::Coils, 8, Scalar::Bool),
            read(Table::Coils, 1, Scalar::Bool),
        ];
        let got = poll(&mut device, &reads, Order::Big).expect("no exception");
        let want = [true, true, true, false, false].map(Value::Bool);
        assert_eq!(got, want);
    }

    #[test]
    fn input_registers_read_as_holding_registers_do() {
        let mut device = Device {
            input_registers: vec![7, 0x3F80, 0],
            ..Device::default()
        };
        let reads = [read(Table::InputRegisters, 1, Scalar::F32)];
        let got = poll(&mut device, &reads, Order::Big).expect("no exception");
        assert_eq!(got, [Value::F32(1.0)]);
    }

    #[test]
    fn a_read_at_the_last_address_reads() {
        let mut holding_registers = vec![0; 1 << 16];
        holding_registers[65532..].copy_from_slice(&[0, 0, 0, 9]);
        let mut device = Device {
            holding_registers,
            coils: vec![true; 1 << 16],
            ..Device::default()
        };
        let reads = [
            read(Table::HoldingRegisters, 65532, Scalar::U64),
            read(Table::HoldingRegisters, 65535, Scalar::U16),
            read(Table::Coils, 65535, Scalar::Bool),
        ];
        let got = poll(&mut device, &reads, Order::Big).expect("no exception");
        assert_eq!(got, [Value::U64(9), Value::U16(9), Value::Bool(true)]);
    }

    #[test]
    fn an_exception_gives_the_exception() {
        let mut device = Device {
            holding_registers: vec![0; 4],
            ..Device::default()
        };
        let reads = [read(Table::HoldingRegisters, 3, Scalar::U32)];
        let got = poll(&mut device, &reads, Order::Big);
        assert_eq!(got, Err(Exception::IllegalAddress));
    }

    fn reply_of(request: &Request, pdu: &'static [u8]) -> Reply<'static> {
        request.decode_reply(pdu).expect("a valid reply")
    }

    #[test]
    #[should_panic(expected = "is not a reply to Read { table: HoldingRegisters, \
                               start: 0, count: 1 }")]
    fn a_reply_of_another_count_panics() {
        let group =
            &groups(&[read(Table::HoldingRegisters, 0, Scalar::U16)], Order::Big)[0];
        let two = Request::Read {
            table: Table::HoldingRegisters,
            start: 0,
            count: 2,
        };
        let _values = group.values(reply_of(&two, &[3, 4, 0, 1, 0, 2]));
    }

    #[test]
    #[should_panic(
        expected = "is not a reply to Read { table: Coils, start: 0, count: 1 }"
    )]
    fn a_reply_of_registers_to_bits_panics() {
        let group = &groups(&[read(Table::Coils, 0, Scalar::Bool)], Order::Big)[0];
        let one = Request::Read {
            table: Table::HoldingRegisters,
            start: 0,
            count: 1,
        };
        let _values = group.values(reply_of(&one, &[3, 2, 0, 1]));
    }

    #[test]
    #[should_panic(expected = "Written is not a reply to")]
    fn a_written_reply_panics() {
        let group = &groups(&[read(Table::Coils, 0, Scalar::Bool)], Order::Big)[0];
        let _values = group.values(Reply::Written);
    }
}

mod new {
    use super::*;

    fn refused(table: Table, sample: Type, text: &str) {
        let error = Read::new(table, 0, sample).expect_err("a refused type");
        assert_eq!(error, Error::Type { table, sample });
        assert_eq!(error.to_string(), text);
    }

    #[test]
    fn bit_tables_hold_only_bool() {
        for table in [Table::Coils, Table::DiscreteInputs] {
            assert!(matches!(
                Read::new(table, 0, Type::Scalar(Scalar::Bool)),
                Ok(Read { count: 1, .. })
            ));
            for scalar in REGISTERS.into_iter().chain([Scalar::U8, Scalar::Uuid]) {
                let sample = Type::Scalar(scalar);
                assert_eq!(
                    Read::new(table, 0, sample),
                    Err(Error::Type { table, sample })
                );
            }
        }
        refused(
            Table::Coils,
            Type::Scalar(Scalar::U16),
            "the coils hold no u16 values",
        );
        refused(
            Table::DiscreteInputs,
            Type::String,
            "the discrete inputs hold no string values",
        );
    }

    #[test]
    fn register_tables_hold_numbers_of_16_32_and_64_bits() {
        let others = [
            Scalar::Bool,
            Scalar::I8,
            Scalar::U8,
            Scalar::Stamp,
            Scalar::Span,
            Scalar::Uuid,
        ];
        for table in [Table::HoldingRegisters, Table::InputRegisters] {
            for scalar in REGISTERS {
                let read =
                    Read::new(table, 0, Type::Scalar(scalar)).expect("a valid read");
                assert_eq!(usize::from(read.count) * 2, scalar.width(), "{scalar:?}");
            }
            for scalar in others {
                let sample = Type::Scalar(scalar);
                assert_eq!(
                    Read::new(table, 0, sample),
                    Err(Error::Type { table, sample })
                );
            }
        }
        refused(
            Table::HoldingRegisters,
            Type::Scalar(Scalar::Bool),
            "the holding registers hold no bool values",
        );
        refused(
            Table::InputRegisters,
            Type::Array {
                element: Scalar::F32,
                len: 3,
            },
            "the input registers hold no f32[3] values",
        );
    }

    #[test]
    fn a_value_past_the_last_address_is_refused() {
        let table = Table::HoldingRegisters;
        let u64 = Type::Scalar(Scalar::U64);
        assert!(Read::new(table, 65532, u64).is_ok_and(|read| read.address == 65532));
        let error = Read::new(table, 65533, u64).expect_err("past the last address");
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
        let u32 = Type::Scalar(Scalar::U32);
        assert!(matches!(
            Read::new(table, 65534, u32),
            Ok(Read { address: 65534, .. })
        ));
        assert_eq!(
            Read::new(table, 65535, u32),
            Err(Error::Range {
                address: 65535,
                count: 2
            })
        );
    }
}

mod groups {
    use super::*;

    fn requests(reads: &[Read]) -> Vec<(Table, u16, u16)> {
        groups(reads, Order::Big)
            .iter()
            .map(|group| match *group.request() {
                Request::Read {
                    table,
                    start,
                    count,
                } => (table, start, count),
                ref other => panic!("a group sends {other:?}"),
            })
            .collect()
    }

    #[test]
    fn go_by_table_then_by_address() {
        let reads = [
            read(Table::InputRegisters, 0, Scalar::U16),
            read(Table::HoldingRegisters, 10, Scalar::U16),
            read(Table::Coils, 5, Scalar::Bool),
            read(Table::HoldingRegisters, 0, Scalar::U16),
            read(Table::DiscreteInputs, 0, Scalar::Bool),
        ];
        assert_eq!(
            requests(&reads),
            [
                (Table::Coils, 5, 1),
                (Table::DiscreteInputs, 0, 1),
                (Table::HoldingRegisters, 0, 1),
                (Table::HoldingRegisters, 10, 1),
                (Table::InputRegisters, 0, 1),
            ]
        );
    }

    #[test]
    fn reads_that_touch_or_overlap_share_a_request() {
        let holding = Table::HoldingRegisters;
        let touch = [read(holding, 1, Scalar::U16), read(holding, 0, Scalar::U16)];
        assert_eq!(requests(&touch), [(holding, 0, 2)]);
        let overlap = [read(holding, 4, Scalar::U16), read(holding, 3, Scalar::U32)];
        assert_eq!(requests(&overlap), [(holding, 3, 2)]);
        let inside = [read(holding, 3, Scalar::U64), read(holding, 4, Scalar::U16)];
        assert_eq!(requests(&inside), [(holding, 3, 4)]);
        let gap = [read(holding, 0, Scalar::U16), read(holding, 2, Scalar::U16)];
        assert_eq!(requests(&gap), [(holding, 0, 1), (holding, 2, 1)]);
    }

    #[test]
    fn reads_go_in_address_order() {
        let holding = Table::HoldingRegisters;
        let reads = [
            read(holding, 4, Scalar::U16),
            read(holding, 3, Scalar::U32),
            read(holding, 3, Scalar::U16),
            read(holding, 3, Scalar::U16),
        ];
        let groups = groups(&reads, Order::Big);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].reads().collect::<Vec<_>>(), [2, 3, 1, 0]);
    }

    #[test]
    fn a_request_holds_at_most_125_registers() {
        let holding = Table::HoldingRegisters;
        let reads = |count: u16| -> Vec<Read> {
            (0..count)
                .map(|address| read(holding, address, Scalar::U16))
                .collect()
        };
        assert_eq!(requests(&reads(124)), [(holding, 0, 124)]);
        assert_eq!(requests(&reads(125)), [(holding, 0, 125)]);
        assert_eq!(
            requests(&reads(126)),
            [(holding, 0, 125), (holding, 125, 1)]
        );
        let wide: Vec<Read> = (0..63)
            .map(|index| read(holding, index * 2, Scalar::U32))
            .collect();
        assert_eq!(requests(&wide), [(holding, 0, 124), (holding, 124, 2)]);
    }

    #[test]
    fn a_request_holds_at_most_2000_bits() {
        let reads = |count: u16| -> Vec<Read> {
            (0..count)
                .map(|address| read(Table::Coils, address, Scalar::Bool))
                .collect()
        };
        assert_eq!(requests(&reads(1999)), [(Table::Coils, 0, 1999)]);
        assert_eq!(requests(&reads(2000)), [(Table::Coils, 0, 2000)]);
        assert_eq!(
            requests(&reads(2001)),
            [(Table::Coils, 0, 2000), (Table::Coils, 2000, 1)]
        );
    }

    #[test]
    fn no_reads_give_no_groups() {
        assert_eq!(groups(&[], Order::Big), []);
    }
}

const SPAN: u16 = 260;

fn any_read() -> impl Strategy<Value = Read> {
    let tables = prop_oneof![
        Just(Table::Coils),
        Just(Table::DiscreteInputs),
        Just(Table::HoldingRegisters),
        Just(Table::InputRegisters),
    ];
    (tables, 0..SPAN, prop::sample::select(REGISTERS.to_vec())).prop_map(
        |(table, address, scalar)| {
            let scalar = if table.bits() { Scalar::Bool } else { scalar };
            read(table, address, scalar)
        },
    )
}

fn any_device() -> impl Strategy<Value = Device> {
    let len = usize::from(SPAN) + 4;
    let bits = || prop::collection::vec(any::<bool>(), len);
    let registers = || prop::collection::vec(any::<u16>(), len);
    (bits(), bits(), registers(), registers()).prop_map(
        |(coils, discrete_inputs, holding_registers, input_registers)| Device {
            coils,
            discrete_inputs,
            holding_registers,
            input_registers,
        },
    )
}

fn any_order() -> impl Strategy<Value = Order> {
    prop_oneof![Just(Order::Big), Just(Order::Little)]
}

fn any_value() -> impl Strategy<Value = Value> {
    prop_oneof![
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

/// The first address and the end of the items of `read`.
fn items(read: Read) -> (u32, u32) {
    (u32::from(read.address), read.end())
}

proptest! {
    #[test]
    fn each_value_round_trips_through_a_device(
        value in any_value(),
        order in any_order(),
        address in 0..SPAN,
        input in any::<bool>(),
        mut device in any_device(),
    ) {
        let words = registers(value, order);
        let (table, tables) = if input {
            (Table::InputRegisters, &mut device.input_registers)
        } else {
            (Table::HoldingRegisters, &mut device.holding_registers)
        };
        let start = usize::from(address);
        tables[start..start + words.len()].copy_from_slice(&words);
        let reads = [read(table, address, value.scalar())];
        let got = poll(&mut device, &reads, order).expect("no exception");
        prop_assert_eq!(bytes(got[0]), bytes(value));
    }

    #[test]
    fn little_reads_the_registers_of_big_in_reverse(
        scalar in prop::sample::select(REGISTERS.to_vec()),
        words in prop::collection::vec(any::<u16>(), 4),
    ) {
        let read = read(Table::HoldingRegisters, 0, scalar);
        let words = &words[..usize::from(read.count)];
        let reversed: Vec<u16> = words.iter().rev().copied().collect();
        let mut big = Device { holding_registers: words.to_vec(), ..Device::default() };
        let mut little = Device { holding_registers: reversed, ..Device::default() };
        let big = poll(&mut big, &[read], Order::Big).expect("no exception");
        let little = poll(&mut little, &[read], Order::Little).expect("no exception");
        prop_assert_eq!(bytes(big[0]), bytes(little[0]));
    }

    #[test]
    fn groups_read_each_read_once_with_no_gap(
        reads in prop::collection::vec(any_read(), 0..120),
        order in any_order(),
        mut device in any_device(),
    ) {
        let groups = groups(&reads, order);
        let mut seen = vec![0; reads.len()];
        let mut last: Option<(Table, u32, u32)> = None;
        for group in &groups {
            let Request::Read { table, start, count } = *group.request() else {
                panic!("a group sends {:?}", group.request());
            };
            let max = if table.bits() { 2000 } else { 125 };
            prop_assert!((1..=max).contains(&count), "{group:?}");
            let (start, end) = (u32::from(start), u32::from(start) + u32::from(count));
            let mut covered = start;
            for index in group.reads() {
                seen[index] += 1;
                let read = reads[index];
                prop_assert_eq!(read.table, table);
                let (first, after) = items(read);
                prop_assert!(start <= first && after <= end, "{read:?} in {group:?}");
                prop_assert!(first <= covered, "a gap before {read:?} in {group:?}");
                covered = covered.max(after);
            }
            prop_assert_eq!(covered, end, "{:?}", group);
            if let Some((before, from, to)) = last
                && before == table
            {
                prop_assert!(
                    start > to || end.max(to) - from > max.into(),
                    "{group:?} could join the group before it"
                );
                prop_assert!(from < start, "{group:?} is out of order");
            }
            last = Some((table, start, end));
        }
        prop_assert!(seen.iter().all(|&count| count == 1), "{seen:?}");
        let got = poll(&mut device, &reads, order).expect("no exception");
        for (read, got) in reads.iter().zip(got) {
            let start = usize::from(read.address);
            let want = match read.table {
                Table::Coils => Value::Bool(device.coils[start]),
                Table::DiscreteInputs => Value::Bool(device.discrete_inputs[start]),
                Table::HoldingRegisters | Table::InputRegisters => {
                    let tables = if read.table == Table::InputRegisters {
                        &device.input_registers
                    } else {
                        &device.holding_registers
                    };
                    let end = start + usize::from(read.count);
                    model(read.scalar, &tables[start..end], order)
                }
            };
            prop_assert_eq!(bytes(got), bytes(want), "{:?}", read);
        }
    }
}
