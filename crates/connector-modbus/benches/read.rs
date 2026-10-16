//! The cost of `read::Group::values`, and of `read::Values::put` and `take`, over
//! one full group, so the time per item is the time per sample: 2000 coils, and full
//! groups of holding registers of each width in each order. Run with
//! `cargo bench -p connector-modbus --bench read`.

use connector_modbus::pdu::{Request, Table};
use connector_modbus::read::{self, Order, Read, Values};
use divan::counter::ItemsCount;
use divan::{Bencher, black_box};
use types::sample::{Scalar, Type};

fn main() {
    divan::main();
}

/// The table, type, number of reads, and order of each case.
const CASES: [(Table, Scalar, u16, Order); 6] = [
    (Table::Coils, Scalar::Bool, 2000, Order::Big),
    (Table::HoldingRegisters, Scalar::U16, 125, Order::Big),
    (Table::HoldingRegisters, Scalar::F32, 62, Order::Big),
    (Table::HoldingRegisters, Scalar::F32, 62, Order::Little),
    (Table::HoldingRegisters, Scalar::F64, 31, Order::Big),
    (Table::HoldingRegisters, Scalar::F64, 31, Order::Little),
];

#[divan::bench(args = [0, 1, 2, 3, 4, 5])]
fn values(bencher: Bencher<'_, '_>, case: usize) {
    let (reads, order) = reads(case);
    let groups = read::groups(&reads, order);
    let [group] = groups.as_slice() else {
        panic!("{case} gives one group");
    };
    let pdu = reply(&group.request());
    let reply = group.request().decode_reply(&pdu).expect("a valid reply");
    bencher
        .counter(ItemsCount::new(reads.len()))
        .bench_local(|| {
            black_box(group)
                .values(black_box(reply))
                .expect("no exception")
                .for_each(|item| {
                    black_box(item);
                });
        });
}

/// The path of the kind: the put of the reply of one full group, and the take.
#[divan::bench(args = [0, 1, 2, 3, 4, 5])]
fn put(bencher: Bencher<'_, '_>, case: usize) {
    let (reads, order) = reads(case);
    let groups = read::groups(&reads, order);
    let [group] = groups.as_slice() else {
        panic!("{case} gives one group");
    };
    let pdu = reply(&group.request());
    let reply = group.request().decode_reply(&pdu).expect("a valid reply");
    let mut values = Values::new(reads.len());
    bencher
        .counter(ItemsCount::new(reads.len()))
        .bench_local(|| {
            let values = black_box(&mut values);
            values
                .put(black_box(group), black_box(reply))
                .expect("no exception");
            values.take().for_each(|value| {
                black_box(value);
            });
        });
}

/// The reads of `case`, which fill one group, and their order.
fn reads(case: usize) -> (Vec<Read>, Order) {
    let (table, scalar, len, order) = CASES[case];
    let width = u16::try_from(scalar.width().div_ceil(2)).expect("a small width");
    let reads = (0..len)
        .map(|at| {
            Read::new(table, at * width, Type::Scalar(scalar)).expect("a valid read")
        })
        .collect();
    (reads, order)
}

/// The PDU of a reply to `request`, a read of coils or holding registers.
fn reply(request: &Request) -> Vec<u8> {
    let Request::Read { table, count, .. } = *request else {
        panic!("a read request");
    };
    let bytes = if table == Table::Coils {
        count.div_ceil(8)
    } else {
        count * 2
    };
    let function = if table == Table::Coils { 1 } else { 3 };
    let mut pdu = vec![function, u8::try_from(bytes).expect("a byte count")];
    pdu.extend((0..bytes).map(|at| u8::try_from(at % 251).expect("a byte")));
    pdu
}
