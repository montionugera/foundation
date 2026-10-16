//! `connector_modbus` never panics on a Modbus TCP stream. A request frame it
//! reads encodes to the same bytes, and a reply to that request reads as many
//! items as the request asked for. The reads of each item of a read request group
//! into that request, and give the items of its reply in order. A device answers every request PDU with a
//! reply that the request reads, or with an exception.
//!
//! Input: one frame, then the PDU of a reply to the request in it.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use connector_modbus::device::Device;
use connector_modbus::pdu::{Exception, Reply, Request, Table};
use connector_modbus::poll::{self, Group, Order, Read};
use connector_modbus::tcp;
use libfuzzer_sys::fuzz_target;
use types::sample::{Scalar, Type, Value};

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
    let items: Vec<Value> = match reply {
        Ok(Reply::Bits(bits)) => {
            assert_eq!((bits.len(), bits.iter().count()), (want, want));
            bits.iter().map(Value::Bool).collect()
        }
        Ok(Reply::Registers(registers)) => {
            assert_eq!((registers.len(), registers.iter().count()), (want, want));
            registers.iter().map(Value::U16).collect()
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
    let scalar = match table {
        Table::Coils | Table::DiscreteInputs => Scalar::Bool,
        Table::HoldingRegisters | Table::InputRegisters => Scalar::U16,
    };
    let reads: Vec<Read> = (start..=start + (count - 1))
        .map(|address| {
            Read::new(table, address, Type::Scalar(scalar))
                .expect("an item of the request")
        })
        .collect();
    let groups = poll::groups(&reads, Order::Big);
    let requests: Vec<&Request> = groups.iter().map(Group::request).collect();
    assert_eq!(requests, [&request], "the reads of one request");
    let Ok(reply) = reply else {
        return;
    };
    match groups[0].values(reply) {
        Ok(values) => {
            let values: Vec<Value> = values
                .enumerate()
                .map(|(at, (read, value))| {
                    assert_eq!(read, at, "the reads in address order");
                    value
                })
                .collect();
            assert_eq!(values, items, "the items of the reply");
        }
        Err(_) => assert!(matches!(reply, Reply::Exception(_)), "{reply:?}"),
    }
});
