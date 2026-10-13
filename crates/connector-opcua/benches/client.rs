//! The cost of one read of `client::Client` against the test server of open62541, each
//! on its own connection manager, over the sim net. Run with `cargo bench -p
//! connector-opcua --bench client`.
//!
//! - `client`: the ns in the polls of the read, the CPU of the client.
//! - `round`: the ns from the first poll of the read to its value, with the server,
//!   the sim net, and the sim scheduler.

use std::future::{Future as _, poll_fn};
use std::pin::pin;
use std::time::{Duration, Instant};

use connector_opcua::bench::client::Server;
use sim::Sim;

const WARMUP: usize = 10;
const ROUNDS: usize = 1000;

fn main() {
    let mut sim = Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let [mut client, mut round] = sim.run_on(&node, bench).expect("the run ends");
    print(&mut [("client", &mut client), ("round", &mut round)]);
}

/// The ns of the polls and of the whole of each timed read.
async fn bench(node: sim::node::Node, _: env::tasks::Tasks) -> [Vec<u64>; 2] {
    let address = node.addresses()[0];
    let body = async |server: &Server| {
        let mut client = server.connect().await;
        let (mut polls, mut rounds) = (Vec::new(), Vec::new());
        for read in 0..WARMUP + ROUNDS {
            let mut future = pin!(client.read());
            let mut spent = Duration::ZERO;
            let start = now();
            poll_fn(|cx| {
                let poll_start = now();
                let poll = future.as_mut().poll(cx);
                spent += now().duration_since(poll_start);
                poll
            })
            .await;
            let round = now().duration_since(start);
            if read >= WARMUP {
                polls.push(nanos(spent));
                rounds.push(nanos(round));
            }
        }
        client.close().await;
        [polls, rounds]
    };
    let rng = node.entropy().rng();
    Server::scope(node.clock(), node.net(), address, rng, body).await
}

#[expect(clippy::disallowed_methods, reason = "a benchmark reads a real clock")]
fn now() -> Instant {
    Instant::now()
}

fn nanos(span: Duration) -> u64 {
    u64::try_from(span.as_nanos()).expect("a read takes under 2^64 ns")
}

#[expect(clippy::print_stdout, reason = "a benchmark prints its results")]
fn print(lines: &mut [(&str, &mut Vec<u64>)]) {
    println!("ns per read over {ROUNDS} reads");
    println!("pN: the read at percentile N");
    println!("{:<12} {:>9} {:>9} {:>9}", "line", "p10", "p50", "p90");
    for (name, nanos) in lines {
        nanos.sort_unstable();
        let at = |percent: usize| nanos[ROUNDS * percent / 100];
        println!("{name:<12} {:>9} {:>9} {:>9}", at(10), at(50), at(90));
    }
}
