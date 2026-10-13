//! The simulated servers of ONE NODE, served by the test process over `os` on
//! loopback.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use connector::cancel::Token;
use connector::http::Client;
use connector_influx::sim::{Field, Store};
use connector_opcua::sim::{Served, Variables, serve};
use env::clock::Clock;
use env::net::tcp::{Config, Listen, Options};
use env::net::{Error, Listener};
use env::tasks::Tasks;
use env::thread::Handle;
use http::Request;
use types::quality::Quality;
use types::time::{Monotonic, Span, Stamp};

/// The options of each stream that a simulated server accepts.
const OPTIONS: Options = Options {
    send_buffer_bytes: 1 << 16,
    recv_buffer_bytes: 1 << 16,
    unsent_bytes_max: NonZeroUsize::new(1 << 14).unwrap(),
    delayed: false,
};

/// The time between two changes of [`Opcua::change`]: 10 Hz.
const TICK: Span = Span::from_nanos(100 * Span::MILLISECOND.nanos());

/// The simulated OPC UA server, with no security. Drop stops it.
#[derive(Debug)]
pub(super) struct Opcua {
    variables: Variables,
    /// The clock of the server.
    clock: Clock,
    /// The wall time at `Monotonic(0)` of `clock`.
    zero: Stamp,
    server: Option<Server>,
}

impl Default for Opcua {
    fn default() -> Self {
        let clock = os::clock();
        let wall = os::wall().expect("read the wall clock");
        #[expect(
            clippy::disallowed_methods,
            reason = "the test maps the times of the server to wall time"
        )]
        let time = wall.now().time;
        let zero = time - (clock.now() - Monotonic(0));
        Self {
            variables: Variables::new(&super::VARIABLES.map(|(node, _)| node)),
            clock,
            zero,
            server: None,
        }
    }
}

/// The wall time of the server at `at`, where `zero` is its wall time at
/// `Monotonic(0)`.
fn wall(zero: Stamp, at: Monotonic) -> Stamp {
    zero + (at - Monotonic(0))
}

/// The simulated InfluxDB, with the database `plant`. Drop stops it.
#[derive(Debug, Default)]
pub(super) struct Influx {
    /// The test `a_dropped_influx_ends_its_shard` counts its owners.
    store: Arc<Mutex<Store>>,
    /// The address of the first [`Influx::serve`].
    address: Option<SocketAddr>,
    /// The panic tests put a shard that panics or blocks here, since no input to
    /// [`Influx::serve`] makes it do so.
    server: Option<Server>,
}

/// The shard of a simulated server. Drop cancels it, and joins it unless the thread
/// panics: then a join can block, and a second panic aborts the test binary.
#[derive(Debug)]
struct Server {
    /// The server in the panic of a join, and the name of its shard.
    name: &'static str,
    /// The token that stops `shard`.
    token: Token,
    shard: Option<Handle>,
}

/// One sample of an OPC UA variable, as the server served it or as InfluxDB holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Sample {
    pub(super) time: Stamp,
    pub(super) value: f64,
    /// The OPC UA status. `None` in InfluxDB, which keeps no status yet (#2510).
    pub(super) quality: Option<Quality>,
}

impl Opcua {
    /// Serves on a free loopback port, and gives the address: the node ids of
    /// [`super::VARIABLES`], each at a first value until [`Opcua::change`].
    pub(super) fn serve(&mut self) -> SocketAddr {
        let listener = listen(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)));
        let address = listener.local();
        let (variables, clock, zero) = (
            Variables::clone(&self.variables),
            self.clock.clone(),
            self.zero,
        );
        let server = Server::start("OPC UA server", move |_, token| async move {
            let mut rng = os::entropy().rng();
            let net = os::net();
            let read = clock.clone();
            let now = move || wall(zero, read.now());
            let until = token.wait();
            serve(clock, now, net, listener, &mut rng, &variables, until).await;
        });
        self.server = Some(server);
        address
    }

    /// Changes each variable `ticks` times at 10 Hz, each change with the status
    /// `quality`, and returns after the last change.
    pub(super) fn change(&self, ticks: usize, quality: Quality) {
        #[expect(
            clippy::disallowed_methods,
            reason = "the test thread is not a thread that env started"
        )]
        let sleep = std::thread::sleep;
        pace(ticks, &self.clock, sleep, || self.variables.change(quality));
    }

    /// Each read that the server answered, by node id, in order: the value and status
    /// of the answer, at the time of the answer.
    pub(super) fn served(&self) -> BTreeMap<String, Vec<Sample>> {
        let sample = |served: &Served| Sample {
            time: wall(self.zero, served.time),
            value: served.value,
            quality: Some(served.quality),
        };
        (self.variables.served().into_iter())
            .map(|(node, served)| (node, served.iter().map(sample).collect()))
            .collect()
    }
}

impl Influx {
    /// Serves, and gives the address: a free loopback port at the first call, and the
    /// same port after [`Influx::stop`]. What it stored stays across a stop.
    pub(super) fn serve(&mut self) -> SocketAddr {
        let listener = listen(self.address.unwrap_or((Ipv4Addr::LOCALHOST, 0).into()));
        let address = listener.local();
        self.address = Some(address);
        let store = Arc::clone(&self.store);
        let server = Server::start("InfluxDB", move |tasks, token| async move {
            let serve =
                connector_influx::sim::serve(listener, tasks, store, "plant".into());
            if let Some(error) = token.race(serve).await {
                panic!("the simulated InfluxDB stopped: {error}");
            }
        });
        self.server = Some(server);
        address
    }

    /// Stops, and panics when the server panicked. A process that holds or takes the
    /// port before the next [`Influx::serve`] makes that serve panic.
    pub(super) fn stop(&mut self) {
        self.server.take().expect("it serves").stop();
    }

    /// Each sample stored in the measurement `plant`, by field key (the channel name),
    /// in time order, each with no quality. Panics on a field that is not a float.
    pub(super) fn stored(&self) -> BTreeMap<String, Vec<Sample>> {
        let store = self.store.lock().expect("the server does not panic");
        let mut stored = BTreeMap::<String, Vec<Sample>>::new();
        for point in store.points("plant", &[]) {
            for (channel, field) in point.fields.iter() {
                let Field::Float(value) = field else {
                    panic!("{channel} holds {field:?} at {}", point.time);
                };
                let sample = Sample {
                    time: point.time,
                    value,
                    quality: None,
                };
                stored.entry(channel.into()).or_default().push(sample);
            }
        }
        stored
    }
}

impl Server {
    /// Starts `main` on a new `os` shard named `name`, with the token that stops it.
    fn start<F>(
        name: &'static str,
        main: impl FnOnce(Tasks, Token) -> F + Send + 'static,
    ) -> Self
    where
        F: Future<Output = ()> + 'static,
    {
        let token = Token::new();
        let stopped = token.clone();
        let shard = start(name, move |tasks| main(tasks, stopped));
        Self {
            name,
            token,
            shard: Some(shard),
        }
    }

    /// Cancels the server and joins its shard. Panics when the server panicked.
    fn stop(mut self) {
        self.join("stops");
    }

    fn join(&mut self, state: &str) {
        self.token.cancel();
        if let Some(shard) = self.shard.take()
            && !std::thread::panicking()
            && let Err(panicked) = shard.join()
        {
            panic!(
                "the simulated {} {state} with no panic: {panicked:?}",
                self.name
            );
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.join("serves");
    }
}

/// Calls `change` `ticks` times at 10 Hz on `clock`, each after a `sleep`. Change
/// `tick` is due `tick` ticks after the call, so a late wake does not move the later
/// changes.
fn pace(
    ticks: usize,
    clock: &Clock,
    mut sleep: impl FnMut(Duration),
    mut change: impl FnMut(),
) {
    let start = clock.now();
    for tick in 1..=ticks {
        sleep(wait(start, tick, clock.now()));
        change();
    }
}

/// The time from `now` to change `tick` of [`pace`], which is due `tick` ticks after
/// `start`: 0 when it is late.
fn wait(start: Monotonic, tick: usize, now: Monotonic) -> Duration {
    let ticks = i64::try_from(tick).expect("a count of ticks fits an i64");
    let due = start + Span::from_nanos(ticks * TICK.nanos());
    let wait = (due - now).max(Span::ZERO);
    Duration::from_nanos(wait.nanos().cast_unsigned())
}

/// Listens on `local`, with the options of each simulated server.
fn listen(local: SocketAddr) -> Listener {
    let listen = Listen {
        local,
        backlog: 128,
        options: OPTIONS,
    };
    (os::net().listen(&listen))
        .unwrap_or_else(|error| panic!("listen on {local}: {error}"))
}

/// Starts `main` on a new `os` shard named `name`.
fn start<F>(name: &str, main: impl FnOnce(Tasks) -> F + Send + 'static) -> Handle
where
    F: Future<Output = ()> + 'static,
{
    let config = env::shards::Config {
        name: name.into(),
        core: None,
    };
    let shards = os::shards().expect("read the cores");
    shards.start(config, main).expect("start the shard")
}

/// Writes `lines` to the database `plant` at `address` from a shard of its own, and
/// gives the status of the answer or the error of the client.
fn write(address: SocketAddr, lines: &'static str) -> String {
    let (send, answer) = mpsc::channel();
    let shard = start("write", move |tasks| async move {
        let client = Client::new(connector::http::Config {
            net: os::net(),
            clock: os::clock(),
            tasks,
            timeout: Span::MINUTE,
            body_max: 1 << 10,
        });
        let request = Request::post(format!("http://{address}/write?db=plant"))
            .body(Bytes::from_static(lines.as_bytes()))
            .expect("a valid request");
        let answer = match client.send(request).await {
            Ok(answer) => answer.status().to_string(),
            Err(error) => error.to_string(),
        };
        send.send(answer).expect("the test reads the answer");
    });
    shard.join().expect("the write runs with no panic");
    answer.try_recv().expect("the shard sent the answer")
}

/// Connects to `address` from a shard of its own, and gives the error of the connect,
/// or `None` when it connects. On macOS, a child that a rig test spawns may hold the
/// port (`os::net`), so only Linux checks a refusal.
fn connect(address: SocketAddr) -> Option<Error> {
    let (send, error) = mpsc::channel();
    let shard = start("connect", move |_| async move {
        let config = Config {
            remote: address,
            options: OPTIONS,
        };
        let connected = os::net().connect(&config).await;
        send.send(connected.err())
            .expect("the test reads the error");
    });
    shard.join().expect("the connect runs with no panic");
    error.try_recv().expect("the shard sent the error")
}

/// The time of each sample of the field `f` that `influx` stored.
fn times(influx: &Influx) -> Vec<Stamp> {
    let stored = influx.stored();
    stored["f"].iter().map(|sample| sample.time).collect()
}

#[test]
fn influx_stores_each_line_written_to_plant_and_keeps_it_after_a_stop() {
    let mut influx = Influx::default();
    let address = influx.serve();
    assert_eq!(write(address, "plant f=1 1\nplant f=2 2"), "204 No Content");
    influx.stop();
    assert_eq!(times(&influx), [Stamp::from_nanos(1), Stamp::from_nanos(2)]);
}

#[test]
fn influx_gives_each_field_of_plant_as_the_samples_of_its_channel() {
    let mut influx = Influx::default();
    let address = influx.serve();
    let lines =
        "plant plant.spike=1,plant.dip=2 1\nplant plant.spike=3 2\nm plant.dip=9 3";
    assert_eq!(write(address, lines), "204 No Content");
    let sample = |time, value| Sample {
        time: Stamp::from_nanos(time),
        value,
        quality: None,
    };
    assert_eq!(
        influx.stored(),
        BTreeMap::from([
            ("plant.dip".into(), vec![sample(1, 2.0)]),
            ("plant.spike".into(), vec![sample(1, 1.0), sample(2, 3.0)]),
        ])
    );
}

#[test]
#[should_panic(expected = "plant.spike holds Integer(1) at ")]
fn influx_panics_on_a_field_that_is_not_a_float() {
    let mut influx = Influx::default();
    let address = influx.serve();
    assert_eq!(write(address, "plant plant.spike=1i 1"), "204 No Content");
    influx.stored();
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "needs SOCK_CLOEXEC and the shutdown of a listener"
)]
fn a_stopped_influx_refuses_and_serves_again_on_its_port_with_what_it_stored() {
    let mut influx = Influx::default();
    let address = influx.serve();
    assert_eq!(write(address, "plant f=1 1"), "204 No Content");
    influx.stop();
    assert_eq!(connect(address), Some(Error::Refused { remote: address }));
    assert_eq!(influx.serve(), address);
    assert_eq!(write(address, "plant f=3 3"), "204 No Content");
    assert_eq!(times(&influx), [Stamp::from_nanos(1), Stamp::from_nanos(3)]);
}

#[test]
fn opcua_changes_at_10_hz_and_records_no_read_that_no_client_made() {
    let mut opcua = Opcua::default();
    opcua.serve();
    let clock = os::clock();
    let before = clock.now();
    opcua.change(3, Quality::GOOD);
    let took = clock.now() - before;
    assert!(took.nanos() >= 3 * TICK.nanos(), "{took:?}");
    let empty = super::VARIABLES.map(|(node, _)| (node.to_owned(), Vec::new()));
    assert_eq!(opcua.served(), BTreeMap::from(empty));
}

#[test]
fn each_change_is_due_its_ticks_after_the_start_whatever_the_wakes() {
    let ms = |n: i64| Span::from_nanos(n * Span::MILLISECOND.nanos());
    // Paces 3 changes on a simulated clock with the first wake `late` late, and gives
    // each sleep and the time of each change after the start.
    let paced = |late: Span| {
        let mut sim = sim::Sim::new(sim::Config::default());
        let clock = sim.node(sim::node::Config::default()).clock();
        let start = clock.now();
        let (mut sleeps, mut changes) = (Vec::new(), Vec::new());
        let sleep = |wait: Duration| {
            let slept =
                Span::from_nanos(i64::try_from(wait.as_nanos()).expect("short"));
            let late = if sleeps.is_empty() { late } else { Span::ZERO };
            sleeps.push(slept);
            let woke = Span::from_nanos(slept.nanos() + late.nanos());
            sim.run_for(woke).expect("the run has no task");
        };
        pace(3, &clock, sleep, || changes.push(clock.now() - start));
        (sleeps, changes)
    };
    let (sleeps, changes) = paced(ms(50));
    assert_eq!(sleeps, [ms(100), ms(50), ms(100)]);
    assert_eq!(changes, [ms(150), ms(200), ms(300)]);
    let (sleeps, changes) = paced(ms(250));
    assert_eq!(sleeps, [ms(100), Span::ZERO, Span::ZERO]);
    assert_eq!(changes, [ms(350); 3]);
}

#[test]
fn a_served_opcua_answers_on_its_address_with_no_http() {
    let mut opcua = Opcua::default();
    let address = opcua.serve();
    let written = write(address, "m f=1 1");
    assert_eq!(written, "the exchange failed: invalid HTTP version parsed");
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "needs SOCK_CLOEXEC and the shutdown of a listener"
)]
fn a_dropped_opcua_stops_each_server_that_it_served() {
    let mut opcua = Opcua::default();
    let first = opcua.serve();
    let second = opcua.serve();
    drop(opcua);
    for address in [first, second] {
        assert_eq!(connect(address), Some(Error::Refused { remote: address }));
    }
}

#[test]
fn a_dropped_influx_ends_its_shard() {
    let mut influx = Influx::default();
    let address = influx.serve();
    assert_eq!(write(address, "m f=1 1"), "204 No Content");
    let store = Arc::clone(&influx.store);
    drop(influx);
    assert_eq!(Arc::strong_count(&store), 1, "the shard holds no store");
    if cfg!(target_os = "linux") {
        assert_eq!(connect(address), Some(Error::Refused { remote: address }));
    }
}

#[test]
fn a_dropped_influx_does_not_join_its_server_when_the_test_panics() {
    let (release, released) = mpsc::channel::<()>();
    let influx = Influx {
        server: Some(Server::start("InfluxDB", move |_, _| async move {
            #[expect(
                clippy::disallowed_methods,
                reason = "the shard blocks until the test releases it"
            )]
            let _waited = released.recv_timeout(Duration::from_secs(30));
        })),
        ..Influx::default()
    };
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _influx = influx;
        panic!("the test broke");
    }));
    unwound.expect_err("the test panics");
    release
        .send(())
        .expect("the drop did not wait for the server");
}

#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "needs SOCK_CLOEXEC and the shutdown of a listener"
)]
fn a_dropped_influx_stops_its_server_when_the_test_panics() {
    let mut influx = Influx::default();
    let address = influx.serve();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _influx = influx;
        panic!("the test broke");
    }));
    unwound.expect_err("the test panics");
    // The drop does not join, so the server stops a moment later.
    crate::rig::wait(
        &os::clock(),
        Span::from_nanos(5_000_000_000),
        "the server stops",
        || {
            let error = connect(address);
            if error == Some(Error::Refused { remote: address }) {
                Ok(())
            } else {
                Err(format!("{error:?}"))
            }
        },
    );
}

#[test]
#[should_panic(expected = "the simulated InfluxDB serves with no panic")]
fn a_dropped_influx_panics_when_its_server_panicked() {
    let influx = Influx {
        server: Some(Server::start("InfluxDB", |_, _| async {
            panic!("the server broke")
        })),
        ..Influx::default()
    };
    drop(influx);
}

#[test]
#[should_panic(expected = "the simulated InfluxDB stops with no panic")]
fn a_stopped_influx_panics_when_its_server_panicked() {
    let mut influx = Influx {
        server: Some(Server::start("InfluxDB", |_, _| async {
            panic!("the server broke")
        })),
        ..Influx::default()
    };
    influx.stop();
}

#[test]
#[should_panic(expected = "the test broke")]
fn a_dropped_influx_does_not_panic_again_when_the_test_panics() {
    let _influx = Influx {
        server: Some(Server::start("InfluxDB", |_, _| async {
            panic!("the server broke")
        })),
        ..Influx::default()
    };
    panic!("the test broke");
}
