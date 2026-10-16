//! The cost of `command::Write::request` for one command value of each table, width,
//! and order. Run with `cargo bench -p connector-modbus --bench command`.

use connector_modbus::command::Write;
use connector_modbus::pdu::Table;
use connector_modbus::poll::Order;
use divan::{Bencher, black_box};
use types::sample::{Type, Value};

fn main() {
    divan::main();
}

/// The table, order, and value of each case.
const CASES: [(Table, Order, Value); 7] = [
    (Table::Coils, Order::Big, Value::Bool(true)),
    (Table::HoldingRegisters, Order::Big, Value::U16(7)),
    (Table::HoldingRegisters, Order::Big, Value::F32(1.5)),
    (Table::HoldingRegisters, Order::Little, Value::F32(1.5)),
    (Table::HoldingRegisters, Order::Big, Value::F64(1.5)),
    (Table::HoldingRegisters, Order::Little, Value::F64(1.5)),
    (
        Table::HoldingRegisters,
        Order::Big,
        Value::U64(0x0102_0304_0506_0708),
    ),
];

#[divan::bench(args = [0, 1, 2, 3, 4, 5, 6])]
fn request(bencher: Bencher<'_, '_>, case: usize) {
    let (table, order, value) = CASES[case];
    let sample = Type::Scalar(value.scalar());
    let mut write = Write::new(table, 10, sample, order).expect("a valid write");
    bencher.bench_local(|| {
        black_box(write.request(black_box(value)));
    });
}
