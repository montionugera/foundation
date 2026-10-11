//! One run of the open62541 event loop with due timers allocates nothing, in C or in
//! Rust, and the fuzz round trip makes a fixed count of allocations, and none after a
//! decode that fails. A read through the connection manager makes a fixed count of
//! allocations, and a drive with nothing ready makes none. The count covers each
//! thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::future::{Future as _, poll_fn};
use std::pin::pin;

use connector_opcua::bench::{Client, Manager, client};
use env::rng::Rng;
use sim::Sim;
use types::time::Span;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

fn main() {
    the_count_holds_the_allocations_of_c();
    a_run_allocates_nothing();
    the_drop_frees_the_client();
    the_fuzz_round_trip_allocates_for_each_step();
    a_failed_decode_stops_the_round_trip();
    a_read_through_the_manager_allocates_a_fixed_count();
    a_read_of_a_client_allocates_a_fixed_count();
}

fn sim() -> (Sim, env::clock::Clock, Rng) {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    (sim, node.clock(), node.entropy().rng())
}

/// The positive control: the copy allocates one entry for each timer. The two loops
/// live at once on this thread, so they take one `Rng`.
fn the_count_holds_the_allocations_of_c() {
    let ((_sim, none, mut rng), (_other, timers, _)) = (sim(), sim());
    let (none, base) = ALLOCATOR.count(|| Client::new(none, &mut rng, 0));
    let (timers, count) = ALLOCATOR.count(|| Client::new(timers, &mut rng, 100));
    assert_eq!(count - base, 100, "the timers of 100 against 0 allocated");
    drop((none, timers));
}

/// Each run for 2 s of simulated time, so the housekeeping timer of the client runs
/// too. After each run, the timers wait 1 ms more.
fn a_run_allocates_nothing() {
    for timers in [1, 100, 10_000] {
        let (mut sim, clock, mut rng) = sim();
        let mut client =
            Client::new(env::clock::Clock::clone(&clock), &mut rng, timers);
        for at in 1..=2000 {
            sim.run_for(Span::MILLISECOND).expect("the run has no task");
            let ((), count) = ALLOCATOR.count(|| client.run());
            assert_eq!(count, 0, "{timers} timers, at {at} ms");
            assert_eq!(
                client.next(),
                Some(clock.now() + Span::MILLISECOND),
                "the run ran the timers, at {at} ms"
            );
        }
    }
}

/// The client of the copy holds this URI when it has no other.
const URI: &[u8] = b"urn:open62541.unconfigured.application";

fn the_drop_frees_the_client() {
    let (_sim, clock, mut rng) = sim();
    let client = Client::new(clock, &mut rng, 0);
    let ((), freed) = ALLOCATOR.freed_holding(URI, || drop(client));
    assert_eq!(freed, 1, "the drop freed the URI of the client");
}

/// A `Variant` (23, as `ffi` is private) of 7 `ExtensionObject` values, then the
/// zeros that #435 needs.
fn the_fuzz_round_trip_allocates_for_each_step() {
    let mut data = vec![23, 0, 0x96, 7, 0, 0, 0];
    data.resize(data.len() + 7 * 4, 0);
    let ((), count) = ALLOCATOR.count(|| connector_opcua::fuzz::decode(&data));
    assert_eq!(count, 10, "the round trip of a Variant");
}

/// A `Boolean` (0) with no byte fails before C allocates, so only the memory of the
/// value is allocated.
fn a_failed_decode_stops_the_round_trip() {
    let ((), count) = ALLOCATOR.count(|| connector_opcua::fuzz::decode(&[0, 0]));
    assert_eq!(count, 1, "a failed decode");
    // A `Variant` of a `Boolean` with no byte: C allocates the `Boolean` first.
    let ((), count) = ALLOCATOR.count(|| connector_opcua::fuzz::decode(&[23, 0, 0x01]));
    assert_eq!(count, 2, "a failed decode after C allocates");
}

/// The delay of the default link.
fn delay() -> Span {
    sim::link::Config::default().delay
}

/// The allocations of each drive of reads 2 to 100 with `idle` idle clients: the ask,
/// the answer of the server, the read of the answer, and a drive with nothing ready.
fn reads(idle: usize) -> Vec<[u64; 4]> {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = move |node: sim::node::Node, _| async move {
        let (clock, address) = (node.clock(), node.addresses()[0]);
        let body = async |manager: &Manager| {
            let mut counts = Vec::new();
            for read in 1..=100 {
                let ((), ask) = ALLOCATOR.count(|| manager.ask());
                clock.sleep(delay()).await;
                let ((), answer) = ALLOCATOR.count(|| manager.drive());
                clock.sleep(delay()).await;
                let ((), receive) = ALLOCATOR.count(|| manager.drive());
                let ((), pass) = ALLOCATOR.count(|| manager.drive());
                assert_eq!(manager.answers(), read, "{idle} idle");
                if read > 1 {
                    counts.push([ask, answer, receive, pass]);
                }
            }
            counts
        };
        let rng = &mut node.entropy().rng();
        Manager::scope(node.clock(), node.net(), address, idle, rng, body).await
    };
    sim.run_on(&node, run).expect("the run ends")
}

/// open62541 allocates for each message it sends and decodes, and the sim net
/// allocates a segment for each write, one in the ask and one in the answer. So a read
/// is not free. The count is the same for each read and each count of idle
/// connections, so a Rust allocation on the path of a message adds one to a count.
fn a_read_through_the_manager_allocates_a_fixed_count() {
    for idle in [0, 15] {
        for (at, counts) in reads(idle).into_iter().enumerate() {
            let read = at + 2;
            assert_eq!(counts, [4, 5, 2, 0], "{idle} idle, read {read}");
        }
    }
}

/// The allocations in the polls of reads 2 to 100 of one client. The server runs on
/// its own manager, outside the polls.
fn client_reads() -> Vec<u64> {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let run = move |node: sim::node::Node, _| async move {
        let address = node.addresses()[0];
        let body = async |server: &client::Server| {
            let mut client = server.connect().await;
            let mut counts = Vec::new();
            for _ in 1..=100 {
                let mut read = pin!(client.read());
                let mut count = 0;
                poll_fn(|cx| {
                    let (poll, allocations) =
                        ALLOCATOR.count(|| read.as_mut().poll(cx));
                    count += allocations;
                    poll
                })
                .await;
                counts.push(count);
            }
            client.close().await;
            counts.split_off(1)
        };
        let rng = node.entropy().rng();
        client::Server::scope(node.clock(), node.net(), address, rng, body).await
    };
    sim.run_on(&node, run).expect("the run ends")
}

/// As through the manager, the ask allocates 4 and the read of the answer 2. Each read
/// is a new drive, whose sleep until the next timer of the loop allocates 1 more.
fn a_read_of_a_client_allocates_a_fixed_count() {
    assert_eq!(client_reads(), [7; 99], "reads 2 to 100");
}
