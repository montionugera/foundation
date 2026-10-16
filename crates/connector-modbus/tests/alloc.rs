//! The decode of the values in a reply and the request of a command value allocate
//! nothing, for each table, each width of a value, and each order. A tick of reads
//! from a device allocates only the segments of the sim net.
//! The count covers each thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::future::{Future as _, poll_fn};
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Mutex;

use connector_modbus::command::Write;
use connector_modbus::device::Device;
use connector_modbus::pdu::Table;
use connector_modbus::read::{self, Order, Read};
use connector_modbus::tcp::{Client, serve};
use env::net::tcp;
use env::tasks::Tasks;
use sim::Sim;
use types::sample::{Scalar, Type, Value};
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    the_count_holds_the_allocations_of_groups();
    a_decode_of_values_allocates_nothing();
    a_request_of_a_command_value_allocates_nothing();
    a_tick_of_reads_allocates_one_segment_for_each_request();
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
    let (groups, allocations) = ALLOCATOR.count(|| read::groups(&reads, Order::Big));
    assert_eq!(groups.len(), 5, "the coils at 0 and 9 do not touch");
    assert!(allocations > 0, "groups allocated nothing");
}

fn a_decode_of_values_allocates_nothing() {
    for order in [Order::Big, Order::Little] {
        for group in read::groups(&reads(), order) {
            let pdu = reply(&group.request());
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

fn a_request_of_a_command_value_allocates_nothing() {
    let values = [
        (Table::Coils, Value::Bool(true)),
        (Table::HoldingRegisters, Value::U16(7)),
        (Table::HoldingRegisters, Value::F32(1.5)),
        (Table::HoldingRegisters, Value::I64(-3)),
    ];
    for order in [Order::Big, Order::Little] {
        for (table, value) in values {
            let sample = Type::Scalar(value.scalar());
            let mut write = Write::new(table, 0, sample, order).expect("a valid write");
            let ((), allocations) = ALLOCATOR.count(|| {
                write.request(value);
            });
            assert_eq!(allocations, 0, "the request of {value:?}");
        }
    }
}

/// The allocations in the polls of ticks 2 to 100 of the reads of [`reads`] from a
/// device on a sim node: an exchange of each group, its put, and the take.
fn ticks() -> Vec<u64> {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = move |node: sim::node::Node, tasks: Tasks| async move {
        let remote = SocketAddr::new(node.addresses()[0], 502);
        let options = tcp::Options {
            send_buffer_bytes: 1 << 12,
            recv_buffer_bytes: 1 << 12,
            unsent_bytes_max: NonZeroUsize::new(1 << 12).expect("not 0"),
            delayed: false,
        };
        let listen = tcp::Listen {
            local: remote,
            backlog: 1,
            options,
        };
        let listener = node.net().listen(&listen).expect("the port is free");
        let device = Device {
            coils: vec![true; 16],
            discrete_inputs: vec![true; 16],
            holding_registers: vec![1; 16],
            input_registers: vec![2; 32],
        };
        tasks.spawn(async move {
            let device = Mutex::new(device);
            serve(listener, 1, &device).await;
        });
        let config = tcp::Config { remote, options };
        let (net, clock) = (node.net(), node.clock());
        let mut client = Client::connect(&net, &config, clock, Span::SECOND)
            .await
            .expect("the device listens");
        let reads = reads();
        let groups = read::groups(&reads, Order::Big);
        let mut values = read::Values::new(reads.len());
        let mut counts = Vec::new();
        for _ in 0..100 {
            let mut tick = pin!(async {
                for group in &groups {
                    let reply = client.exchange(1, &group.request()).await;
                    values
                        .put(group, reply.expect("a reply"))
                        .expect("no exception");
                }
                values.take().count()
            });
            let mut count = 0;
            let taken = poll_fn(|cx| {
                let (poll, allocations) = ALLOCATOR.count(|| tick.as_mut().poll(cx));
                count += allocations;
                poll
            })
            .await;
            assert_eq!(taken, reads.len(), "a value of each read");
            counts.push(count);
        }
        counts.split_off(1)
    };
    sim.run_on(&node, run).expect("the run ends")
}

/// The sim net allocates a segment for each write, so each request allocates one,
/// and the client, the decode, and the values allocate nothing.
fn a_tick_of_reads_allocates_one_segment_for_each_request() {
    let requests = u64::try_from(read::groups(&reads(), Order::Big).len()).expect("5");
    assert_eq!(ticks(), [requests; 99], "ticks 2 to 100");
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
