//! `connector_modbus` never panics on a Modbus TCP stream. A request frame it
//! reads encodes to the same bytes, and a reply to that request reads as many
//! items as the request asked for. Reads that tile a read request group into that
//! request, and give the values of its reply in order. A device answers every
//! request PDU with a reply that the request reads, or with an exception.
//!
//! Input: one frame, then the PDU of a reply to the request in it. Bit 0 of the
//! transaction picks the register order, and bits 1 to 3 the type of the register
//! reads.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use connector_modbus::device::Device;
use connector_modbus::pdu::{Exception, Reply, Request, Table};
use connector_modbus::poll::{self, Group, Order, Read};
use connector_modbus::tcp;
use libfuzzer_sys::fuzz_target;
use types::sample::{Scalar, Type};

fuzz_target!(|bytes: &[u8]| {
    let Ok(Some(frame)) = tcp::decode(bytes) else {
        return;
    };
    let mut device = Device {
        coils: vec![false; 300],
        discrete_inputs: vec![true; 300],
        holding_registers: vec![1; 300],
        input_registers: vec![2; 300],
    };
    let mut answer = Vec::new();
    device.answer(frame.pdu, &mut answer);
    let Ok(request) = Request::decode(frame.pdu) else {
        assert!(
            matches!(answer[..], [function, 1..=3] if function & 0x80 != 0),
            "{answer:x?} is not an exception"
        );
        return;
    };
    match request.decode_reply(&answer) {
        Ok(Reply::Exception(exception)) => {
            assert_eq!(exception, Exception::IllegalAddress);
        }
        Ok(_) => {}
        Err(error) => panic!("the device's reply does not read: {error}"),
    }

    let mut out = Vec::new();
    tcp::encode(frame.header, &request, &mut out).expect("a decoded request encodes");
    assert_eq!(out, bytes[..frame.len], "the frame changed");

    let want = match &request {
        Request::Read { count, .. } => usize::from(*count),
        _ => 0,
    };
    let reply = request.decode_reply(&bytes[frame.len..]);
    let items: Vec<u16> = match reply {
        Ok(Reply::Bits(bits)) => {
            assert_eq!((bits.len(), bits.iter().count()), (want, want));
            bits.iter().map(u16::from).collect()
        }
        Ok(Reply::Registers(registers)) => {
            assert_eq!((registers.len(), registers.iter().count()), (want, want));
            registers.iter().collect()
        }
        _ => Vec::new(),
    };

    let Request::Read {
        table,
        start,
        count,
    } = request
    else {
        return;
    };
    let transaction = usize::from(frame.header.transaction);
    let order = [Order::Big, Order::Little][transaction % 2];
    let scalar = match table {
        Table::Coils | Table::DiscreteInputs => Scalar::Bool,
        Table::HoldingRegisters | Table::InputRegisters => {
            REGISTERS[transaction / 2 % 8]
        }
    };
    let (reads, slots) = tile(table, start, count, scalar);
    let groups = poll::groups(&reads, order);
    let requests: Vec<&Request> = groups.iter().map(Group::request).collect();
    assert_eq!(requests, [&request], "the reads of one request");
    let Ok(reply) = reply else {
        return;
    };
    match groups[0].values(reply) {
        Ok(values) => {
            let got: Vec<(usize, Scalar, Vec<u8>)> = values
                .map(|(read, value)| {
                    let mut sample = vec![0; value.scalar().width()];
                    value.write(&mut sample);
                    (read, value.scalar(), sample)
                })
                .collect();
            let want: Vec<(usize, Scalar, Vec<u8>)> = slots
                .iter()
                .enumerate()
                .map(|(read, &(at, scalar))| {
                    (read, scalar, sample(&items, at, scalar, order))
                })
                .collect();
            assert_eq!(got, want, "the values of the reply in address order");
        }
        Err(_) => assert!(matches!(reply, Reply::Exception(_)), "{reply:?}"),
    }
});

/// The register types, in the order that the transaction picks them.
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

/// Reads of `scalar` from `start` that fill the `count` items, with `u16` reads for
/// the items that are left. Also the offset and type of each read.
fn tile(
    table: Table,
    start: u16,
    count: u16,
    scalar: Scalar,
) -> (Vec<Read>, Vec<(usize, Scalar)>) {
    let width = scalar.width().div_ceil(2);
    let (mut reads, mut slots) = (Vec::new(), Vec::new());
    let mut at = 0;
    while at < usize::from(count) {
        let sample = if at + width <= usize::from(count) {
            scalar
        } else {
            Scalar::U16
        };
        let address = start + u16::try_from(at).expect("an offset in the request");
        reads.push(
            Read::new(table, address, Type::Scalar(sample))
                .expect("a read in the request"),
        );
        slots.push((at, sample));
        at += sample.width().div_ceil(2);
    }
    (reads, slots)
}

/// The little-endian bytes of the `scalar` value at offset `at` of `items`. The first
/// register holds the high word in `Order::Big`, and the low word in `Order::Little`.
fn sample(items: &[u16], at: usize, scalar: Scalar, order: Order) -> Vec<u8> {
    if scalar == Scalar::Bool {
        return vec![u8::from(items[at] != 0)];
    }
    let mut words = items[at..at + scalar.width() / 2].to_vec();
    if order == Order::Big {
        words.reverse();
    }
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}
