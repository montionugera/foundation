//! The simulated servers of ONE NODE, served by the test process over `os` on
//! loopback.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use connector::cancel::Token;
use connector::http::Client;
use connector_influx::sim::{Field, Store};
use connector_opcua::sim::{Served, Variables, serve};
use env::clock::Clock;
use env::net::tcp::{Listen, Options};
use env::thread::Handle;
use http::Request;
use types::quality::Quality;
use types::time::{Span, Stamp};

/// The options of each stream that a simulated server accepts.
const OPTIONS: Options = Options {
    send_buffer_bytes: 1 << 16,
    recv_buffer_bytes: 1 << 16,
    unsent_bytes_max: NonZeroUsize::new(1 << 14).unwrap(),
    delayed: false,
};

/// The variables of [`Opcua`].
const NODES: [&str; 3] = [
    "ns=3;s=SpikeData",
    "ns=3;s=DipData",
    "ns=3;s=PositiveTrendData",
];

/// The time between two changes of [`Opcua::change`]: 10 Hz.
const TICK: Span = Span::from_nanos(100 * Span::MILLISECOND.nanos());

/// The simulated OPC UA server, with no security. Drop stops it.
#[derive(Debug)]
pub(super) struct Opcua {
    variables: Variables,
    serving: Option<Serving>,
}

/// The server of [`Opcua::serve`].
#[derive(Debug)]
struct Serving {
    clock: Clock,
    /// The wall time at `Monotonic(0)` of `clock`.
    zero: Stamp,
    /// The token that stops `shard`.
    stop: Token,
    shard: Handle,
}

impl Default for Opcua {
    fn default() -> Self {
        Self {
            variables: Variables::new(&NODES),
            serving: None,
        }
    }
}

/// The simulated InfluxDB, with the database `plant`. Drop stops it.
#[derive(Debug, Default)]
pub(super) struct Influx {
    /// The test `a_dropped_influx_ends_its_shard` counts its owners.
    store: Arc<Mutex<Store>>,
    /// The address of the first [`Influx::serve`].
    address: Option<SocketAddr>,
    /// The token that stops the shard that serves, and that shard. The panic tests put
    /// a shard that panics or blocks here, since no input to [`Influx::serve`] makes
    /// it do so.
    serving: Option<(Token, Handle)>,
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
    /// Serves on a free loopback port, and gives the address: `ns=3;s=SpikeData`,
    /// `ns=3;s=DipData`, and `ns=3;s=PositiveTrendData`, each at a first value until
    /// [`Opcua::change`].
    pub(super) fn serve(&mut self) -> SocketAddr {
        let clock = os::clock();
        let wall = os::wall().expect("read the wall clock");
        #[expect(
            clippy::disallowed_methods,
            reason = "the test maps the times of the server to wall time"
        )]
        let time = wall.now().time;
        let zero = time - Span::from_nanos(clock.now().0.cast_signed());
        let local = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let listen = Listen {
            local,
            backlog: 128,
            options: OPTIONS,
        };
        let listener = (os::net().listen(&listen))
            .unwrap_or_else(|error| panic!("listen on {local}: {error}"));
        let address = listener.local();
        let (stop, variables) = (Token::new(), Variables::clone(&self.variables));
        let (stopped, moved) = (stop.clone(), Clock::clone(&clock));
        let shard = start("opcua", move |_| async move {
            let mut rng = os::entropy().rng();
            let net = os::net();
            serve(moved, net, listener, &mut rng, &variables, stopped.wait()).await;
        });
        self.serving = Some(Serving {
            clock,
            zero,
            stop,
            shard,
        });
        address
    }

    /// Changes each variable `ticks` times at 10 Hz, each change with the status
    /// `quality`, and returns after the last change.
    pub(super) fn change(&self, ticks: usize, quality: Quality) {
        let clock = &self.serving.as_ref().expect("it serves").clock;
        let mut at = clock.now();
        for _ in 0..ticks {
            at = at + TICK;
            let wait = (at - clock.now()).nanos().max(0);
            #[expect(
                clippy::disallowed_methods,
                reason = "the test thread is not a thread that env started"
            )]
            std::thread::sleep(std::time::Duration::from_nanos(wait.cast_unsigned()));
            self.variables.change(quality);
        }
    }

    /// Each read that the server answered, by node id, in order: the value and status
    /// of the answer, at the time of the answer.
    pub(super) fn served(&self) -> BTreeMap<String, Vec<Sample>> {
        let zero = self.serving.as_ref().expect("it serves").zero;
        let sample = |served: &Served| Sample {
            time: zero + Span::from_nanos(served.time.0.cast_signed()),
            value: served.value,
            quality: Some(served.quality),
        };
        (self.variables.served().into_iter())
            .map(|(node, served)| (node, served.iter().map(sample).collect()))
            .collect()
    }
}

impl Drop for Opcua {
    fn drop(&mut self) {
        // While the thread panics, a join can block and a second panic aborts the
        // test binary.
        if let Some(Serving { stop, shard, .. }) = self.serving.take() {
            stop.cancel();
            if !std::thread::panicking() {
                shard
                    .join()
                    .expect("the simulated OPC UA server serves with no panic");
            }
        }
    }
}

impl Influx {
    /// Serves, and gives the address: a free loopback port at the first call, and the
    /// same port after [`Influx::stop`]. What it stored stays across a stop.
    pub(super) fn serve(&mut self) -> SocketAddr {
        let local = self.address.unwrap_or((Ipv4Addr::LOCALHOST, 0).into());
        let listen = Listen {
            local,
            backlog: 128,
            options: OPTIONS,
        };
        let listener = (os::net().listen(&listen))
            .unwrap_or_else(|error| panic!("listen on {local}: {error}"));
        let address = listener.local();
        self.address = Some(address);
        let (stop, store) = (Token::new(), Arc::clone(&self.store));
        let stopped = stop.clone();
        let shard = start("influx", move |tasks| async move {
            let serve =
                connector_influx::sim::serve(listener, tasks, store, "plant".into());
            if let Some(error) = stopped.race(serve).await {
                panic!("the simulated InfluxDB stopped: {error}");
            }
        });
        self.serving = Some((stop, shard));
        address
    }

    /// Stops, and panics when the server panicked. A process that holds or takes the
    /// port before the next [`Influx::serve`] makes that serve panic.
    pub(super) fn stop(&mut self) {
        let shard = self.cancel().expect("it serves");
        shard
            .join()
            .expect("the simulated InfluxDB stops with no panic");
    }

    /// Cancels the server, and gives its shard to join, or `None` when it does not
    /// serve.
    fn cancel(&mut self) -> Option<Handle> {
        let (stop, shard) = self.serving.take()?;
        stop.cancel();
        Some(shard)
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

impl Drop for Influx {
    fn drop(&mut self) {
        // While the thread panics, a join can block and a second panic aborts the
        // test binary. The server stops on its own.
        if let Some(shard) = self.cancel()
            && !std::thread::panicking()
        {
            shard
                .join()
                .expect("the simulated InfluxDB serves with no panic");
        }
    }
}

/// Starts `main` on a new `os` shard named `name`.
fn start<F>(
    name: &str,
    main: impl FnOnce(env::tasks::Tasks) -> F + Send + 'static,
) -> Handle
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
    write_within(address, lines, Span::MINUTE)
}

/// [`write`], with a client timeout of `timeout`.
fn write_within(address: SocketAddr, lines: &'static str, timeout: Span) -> String {
    let (send, answer) = mpsc::channel();
    let shard = start("write", move |tasks| async move {
        let client = Client::new(connector::http::Config {
            net: os::net(),
            clock: os::clock(),
            tasks,
            timeout,
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

/// What [`write`] gives when no server listens at `address`. On macOS, a child that a
/// rig test spawns may hold the port (`os::net`), so only Linux checks a refusal.
fn refused(address: SocketAddr) -> String {
    format!("the connect failed: {address} refused the connection")
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
    assert_eq!(write(address, "plant f=2 2"), refused(address));
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
    let empty = NODES.map(|node| (node.to_owned(), Vec::new()));
    assert_eq!(opcua.served(), BTreeMap::from(empty));
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
        assert_eq!(write(address, "m f=2 2"), refused(address));
    }
}

#[test]
fn a_dropped_influx_does_not_join_its_server_when_the_test_panics() {
    let (release, released) = mpsc::channel::<()>();
    let mut influx = Influx::default();
    let shard = start("influx", move |_| async move {
        #[expect(
            clippy::disallowed_methods,
            reason = "the shard blocks until the test releases it"
        )]
        let _waited = released.recv_timeout(std::time::Duration::from_secs(30));
    });
    influx.serving = Some((Token::new(), shard));
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
            let written = write_within(address, "m f=1 1", Span::SECOND);
            if written == refused(address) {
                Ok(())
            } else {
                Err(written)
            }
        },
    );
}

#[test]
#[should_panic(expected = "the simulated InfluxDB serves with no panic")]
fn a_dropped_influx_panics_when_its_server_panicked() {
    let mut influx = Influx::default();
    let shard = start("influx", |_| async { panic!("the server broke") });
    influx.serving = Some((Token::new(), shard));
    drop(influx);
}

#[test]
#[should_panic(expected = "the simulated InfluxDB stops with no panic")]
fn a_stopped_influx_panics_when_its_server_panicked() {
    let mut influx = Influx::default();
    let shard = start("influx", |_| async { panic!("the server broke") });
    influx.serving = Some((Token::new(), shard));
    influx.stop();
}

#[test]
#[should_panic(expected = "the test broke")]
fn a_dropped_influx_does_not_panic_again_when_the_test_panics() {
    let mut influx = Influx::default();
    let shard = start("influx", |_| async { panic!("the server broke") });
    influx.serving = Some((Token::new(), shard));
    panic!("the test broke");
}
