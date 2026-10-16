//! The decode of the values in a reply allocates nothing, for each table and type.
//! The count covers each thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use connector_modbus::pdu::Table;
use connector_modbus::poll::{self, Order, Read};
use types::sample::{Scalar, Type};

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    the_count_holds_the_allocations_of_groups();
    a_decode_of_values_allocates_nothing();
}

fn reads() -> Vec<Read> {
    let read = |table, address, scalar| {
        Read::new(table, address, Type::Scalar(scalar)).expect("a valid read")
    };
    vec![
        read(Table::Coils, 0, Scalar::Bool),
        read(Table::Coils, 9, Scalar::Bool),
        read(Table::DiscreteInputs, 3, Scalar::Bool),
        read(Table::HoldingRegisters, 0, Scalar::U16),
        read(Table::HoldingRegisters, 1, Scalar::I32),
        read(Table::HoldingRegisters, 3, Scalar::F64),
        read(Table::InputRegisters, 10, Scalar::I16),
        read(Table::InputRegisters, 11, Scalar::F32),
        read(Table::InputRegisters, 13, Scalar::U64),
    ]
}

/// The positive control.
fn the_count_holds_the_allocations_of_groups() {
    let reads = reads();
    let (groups, allocations) = ALLOCATOR.count(|| poll::groups(&reads, Order::Big));
    assert_eq!(groups.len(), 5, "the coils at 0 and 9 do not touch");
    assert!(allocations > 0, "groups allocated nothing");
}

fn a_decode_of_values_allocates_nothing() {
    for order in [Order::Big, Order::Little] {
        for group in poll::groups(&reads(), order) {
            let pdu = reply(group.request());
            let reply = group.request().decode_reply(&pdu).expect("a valid reply");
            let (sum, allocations) = ALLOCATOR.count(|| {
                group
                    .values(reply)
                    .expect("no exception")
                    .map(|(read, _)| read)
                    .sum::<usize>()
            });
            assert_eq!(sum, group.reads().sum::<usize>(), "the reads of the group");
            assert_eq!(allocations, 0, "the values of {:?}", group.request());
        }
    }
}

/// The PDU of a reply of zeros to `request`.
fn reply(request: &connector_modbus::pdu::Request) -> Vec<u8> {
    let connector_modbus::pdu::Request::Read { table, count, .. } = *request else {
        panic!("a read request")
    };
    let (function, bytes) = match table {
        Table::Coils => (1, count.div_ceil(8)),
        Table::DiscreteInputs => (2, count.div_ceil(8)),
        Table::HoldingRegisters => (3, count * 2),
        Table::InputRegisters => (4, count * 2),
    };
    let mut pdu = vec![function, u8::try_from(bytes).expect("a byte count")];
    pdu.resize(usize::from(bytes) + 2, 0);
    pdu
}
