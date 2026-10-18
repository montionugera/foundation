//! The Rust SDK against a node process.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use env::tasks::{Group, Tasks};
use sdk::write::{Config, Outcome, Refusal as Refused, Writer};
use sdk::{Client, Error, Node, Subject};
use types::ed25519::PrivateKey;
use types::frame::Range;
use types::name::Selector;
use types::sample::{Scalar, Type, Value};
use types::time::Stamp;
use wire::hub::client::{Mode, Refusal};

use crate::rig::Rig;

/// The bytes of the file `name` of the data directory of `rig`, once it holds `tag`.
/// Checks the CRC32C of the file.
fn read<const LEN: usize>(rig: &Rig, name: &str, tag: &[u8]) -> [u8; LEN] {
    let path = rig.dir.join("foundation-data/data").join(name);
    let bytes = rig.wait(&format!("the node writes {name}"), || held(&path, tag));
    let body = LEN - 4;
    assert_eq!(
        crc32c::crc32c(&bytes[..body]).to_le_bytes(),
        bytes[body..],
        "the CRC32C of {name}"
    );
    bytes
}

/// The bytes at `path` when they are `LEN` long and start with `tag`.
#[expect(
    clippy::disallowed_methods,
    reason = "the test reads a file of a node process"
)]
fn held<const LEN: usize>(path: &Path, tag: &[u8]) -> Result<[u8; LEN], String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    match <[u8; LEN]>::try_from(bytes) {
        Ok(bytes) if bytes.starts_with(tag) => Ok(bytes),
        Ok(bytes) => Err(format!("{bytes:?}")),
        Err(bytes) => Err(format!("{} bytes", bytes.len())),
    }
}

/// The node of `rig`, with an unspecified IP as loopback, and its admin, from the
/// files `address` and `admin.key`.
fn admin(rig: &Rig) -> (Node, Subject) {
    let address: [u8; 99] = read(rig, "address", b"foundation/address/1");
    let key: [u8; 54] = read(rig, "admin.key", b"foundation/admin/1");
    let port = u16::from_be_bytes([address[85], address[86]]);
    let ip = match address[68] {
        4 => IpAddr::from(<[u8; 4]>::try_from(&address[69..73]).unwrap()),
        6 => IpAddr::from(<[u8; 16]>::try_from(&address[69..85]).unwrap()),
        family => panic!("the address has family {family}"),
    };
    let ip = match ip {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv6Addr::LOCALHOST.into(),
        ip => ip,
    };
    let node = u128::from_be_bytes(address[20..36].try_into().unwrap());
    let public_key = <[u8; 32]>::try_from(&address[36..68]).unwrap();
    let node = Node {
        addresses: vec![SocketAddr::new(ip, port)],
        key: types::node::Key::from_u128(node),
        public_key: types::ed25519::PublicKey::new(public_key).unwrap(),
    };
    let subject = Subject {
        name: spec::founding::admin(),
        private_key: PrivateKey(key[18..50].try_into().unwrap()),
    };
    (node, subject)
}

/// The channels of the tests of writers: the index `plant.time`, and `plant.a` and
/// `plant.b` of `i64` on it, homed at the node `edge`.
const CHANNELS: &str = concat!(
    "channel \"plant.time\" { kind = \"index\" }\n",
    "channel \"plant.a\" {\n  data_type = \"i64\"\n  index = \"plant.time\"\n}\n",
    "channel \"plant.b\" {\n  data_type = \"i64\"\n  index = \"plant.time\"\n}\n",
    "placement \"plant.home\" {\n  select = \"plant.*\"\n  home = \"edge\"\n}\n",
);

/// A rig whose node runs with [`CHANNELS`] applied, its node, and its admin. Waits
/// until the hub of the node knows the channels, which it learns after the apply.
fn planted() -> (Rig, Node, Subject) {
    let mut rig = Rig::new();
    rig.config(CHANNELS);
    rig.start();
    rig.apply();
    let (node, subject) = admin(&rig);
    let client = Client::connect(&node, subject.clone()).unwrap();
    rig.wait("the hub knows the channels", || {
        let channels = vec!["plant.a".parse().unwrap(), "plant.b".parse().unwrap()];
        let writer = client.open_writer(Config::new(channels));
        writer.and_then(Writer::close).map_err(|e| e.to_string())
    });
    assert_eq!(client.close(), Ok(()));
    (rig, node, subject)
}

/// The samples of one frame, by channel name, each as `i64`.
type Samples = BTreeMap<String, Vec<i64>>;

/// Opens a reader of each channel that `select` matches at `node` as `subject`, in a
/// session on a thread of its own, and gives the samples of each of the next `frames`
/// frames that it reads. The reader gets only the frames that commit after this call
/// returns.
fn reader(
    node: &Node,
    subject: &Subject,
    select: &str,
    frames: usize,
) -> mpsc::Receiver<Samples> {
    let (node, subject) = (node.clone(), subject.clone());
    let select = Selector::new([select]).unwrap();
    let (sender, opened) = mpsc::sync_channel(1);
    let (samples, read) = mpsc::channel();
    let config = env::shards::Config {
        name: "reader".into(),
        core: None,
    };
    let main = move |tasks: Tasks| async move {
        let group = Group::new(tasks);
        let session = session(node, subject, group.tasks().clone()).await;
        let mut reader = session.reader(select, Mode::Complete).await.unwrap();
        sender.send(()).unwrap();
        for _ in 0..frames {
            let frame = reader.next().await.unwrap();
            let count = usize::try_from(frame.range(0).unwrap().count).unwrap();
            let read = frame.iter().map(|(entry, bytes)| {
                let place = &reader.places()[entry];
                let mut out = Vec::new();
                codec::decode(place.data_type, count, bytes, &mut out).unwrap();
                let (chunks, _) = out.as_chunks::<8>();
                let values = chunks.iter().map(|chunk| i64::from_le_bytes(*chunk));
                (place.name.to_string(), values.collect())
            });
            samples.send(read.collect()).unwrap();
        }
        drop((reader, session));
        group.ended().await;
    };
    drop(os::shards().unwrap().start(config, main).unwrap());
    wait(&opened);
    read
}

/// The next of `answers`. Panics when its sender drops, such as at a panic of the
/// reader's thread, or when 30 s pass first.
#[expect(
    clippy::disallowed_methods,
    reason = "the test waits for the thread of a reader in real time"
)]
fn wait<T>(answers: &mpsc::Receiver<T>) -> T {
    answers
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("the reader answers")
}

/// A session with `node` as `subject`, as the client of `sdk` makes it.
async fn session(node: Node, subject: Subject, tasks: Tasks) -> program::Session {
    let budget = |bytes| block::Config::new(bytes).unwrap();
    let clock = os::clock();
    let entropy = os::entropy();
    let pool = Rc::new(block::Pool::heap(budget(1 << 21)));
    let transport = transport::Client::new(transport::client::Config {
        net: os::net(),
        clock: clock.clone(),
        entropy: entropy.clone(),
        tasks: tasks.clone(),
        pool: Rc::clone(&pool),
    })
    .unwrap();
    let config = program::Config {
        via: node.key,
        node: node.public_key,
        addresses: node
            .addresses
            .into_iter()
            .map(transport::Address::Udp)
            .collect(),
        subject: subject.name,
        key: subject.private_key,
        clock,
        entropy,
        tasks,
        pool,
        frames: Rc::new(block::Pool::heap(budget(64 << 10))),
    };
    program::Session::connect(&transport, config).await.unwrap()
}

/// The place of the channel `name` of `writer`.
fn place(writer: &Writer, name: &str) -> usize {
    let name = name.parse().unwrap();
    let places = writer.places();
    places.iter().position(|place| place.name == name).unwrap()
}

/// The outcome of a frame of `count` samples that the node took at `seq`.
fn taken(seq: u64, count: u32) -> Outcome {
    Outcome::Taken {
        range: Range { seq, count },
    }
}

#[test]
fn a_client_connects_to_a_node_and_connects_again_after_its_close() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    let client = Client::connect(&node, subject.clone()).unwrap();
    assert_eq!(client.close(), Ok(()));
    let client = Client::connect(&node, subject).unwrap();
    drop(client);
    rig.stop();
}

/// A UDP relay to `node` for one client. Gives its address and the count of the
/// datagrams that the client sent through it.
#[expect(
    clippy::disallowed_methods,
    reason = "a relay between a client and a node"
)]
fn relay(node: SocketAddr) -> (SocketAddr, Arc<AtomicUsize>) {
    let front = UdpSocket::bind((node.ip(), 0)).unwrap();
    let back = UdpSocket::bind((node.ip(), 0)).unwrap();
    back.connect(node).unwrap();
    let address = front.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let (to_client, from_node) =
        (front.try_clone().unwrap(), back.try_clone().unwrap());
    let (client, counted) = std::sync::mpsc::channel();
    let sent = Arc::clone(&count);
    std::thread::spawn(move || {
        let mut buffer = vec![0; 1 << 16];
        while let Ok((len, from)) = front.recv_from(&mut buffer) {
            if sent.fetch_add(1, Ordering::SeqCst) == 0 {
                client.send(from).unwrap();
            }
            back.send(&buffer[..len]).unwrap();
        }
    });
    std::thread::spawn(move || {
        let client = counted.recv().unwrap();
        let mut buffer = vec![0; 1 << 16];
        while let Ok(len) = from_node.recv(&mut buffer) {
            to_client.send_to(&buffer[..len], client).unwrap();
        }
    });
    (address, count)
}

#[test]
fn the_close_of_a_client_sends_the_close_to_the_node() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    let (front, count) = relay(node.addresses[0]);
    let node = Node {
        addresses: vec![front],
        ..node
    };
    let client = Client::connect(&node, subject).unwrap();
    let before = count.load(Ordering::SeqCst);
    assert_eq!(client.close(), Ok(()));
    rig.wait("a datagram after the close", || {
        let after = count.load(Ordering::SeqCst);
        if after > before {
            Ok(())
        } else {
            Err(format!("{after} datagrams"))
        }
    });
    rig.stop();
}

/// The node sends its close before it exits. No call of the client shows that the
/// client took it, so the test gives the client 1 s on loopback.
#[test]
fn the_close_of_a_client_gives_the_cause_when_the_node_closed_the_session() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    let client = Client::connect(&node, subject).unwrap();
    rig.stop();
    #[expect(
        clippy::disallowed_methods,
        reason = "the client takes the close of a node process in real time"
    )]
    std::thread::sleep(std::time::Duration::from_secs(1));
    let closed = transport::Error::PeerClosed {
        code: transport::Code(0),
    };
    assert_eq!(
        client.close(),
        Err(Error::Session(program::Error::Transport(closed)))
    );
}

#[test]
fn a_client_dials_the_next_address_when_the_first_does_not_answer() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    #[expect(clippy::disallowed_methods, reason = "a socket that never reads")]
    let silent = std::net::UdpSocket::bind((node.addresses[0].ip(), 0)).unwrap();
    let (front, count) = relay(node.addresses[0]);
    let node = Node {
        addresses: vec![silent.local_addr().unwrap(), front],
        ..node
    };
    let client = Client::connect(&node, subject).unwrap();
    assert_ne!(count.load(Ordering::SeqCst), 0);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

#[test]
fn a_client_with_a_wrong_public_key_gets_the_error_of_the_handshake() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    let peer = PrivateKey([7; 32]).public();
    let node = Node {
        public_key: peer,
        ..node
    };
    let error = Client::connect(&node, subject).unwrap_err();
    let unreachable = transport::Error::Unreachable {
        peer,
        attempts: vec![(
            transport::Address::Udp(node.addresses[0]),
            transport::Error::Authentication { expected: peer },
        )],
    };
    assert_eq!(
        error,
        Error::Session(program::Error::Transport(unreachable))
    );
    rig.stop();
}

#[test]
fn a_client_of_a_subject_that_the_spec_does_not_list_is_refused() {
    let mut rig = Rig::new();
    rig.start();
    let (node, subject) = admin(&rig);
    let subject = Subject {
        name: "plant.operator".parse().unwrap(),
        ..subject
    };
    let error = Client::connect(&node, subject).unwrap_err();
    assert_eq!(
        error,
        Error::Session(program::Error::Refused(Refusal::Refused))
    );
    assert_eq!(error.to_string(), Refusal::Refused.to_string());
    rig.stop();
}

#[test]
fn a_client_of_an_address_that_no_node_reads_is_unreachable() {
    #[expect(clippy::disallowed_methods, reason = "a socket that never reads")]
    let silent = std::net::UdpSocket::bind((Ipv6Addr::LOCALHOST, 0)).unwrap();
    let address = silent.local_addr().unwrap();
    let peer = PrivateKey([7; 32]).public();
    let node = Node {
        addresses: vec![address],
        key: types::node::Key::from_u128(1),
        public_key: peer,
    };
    let subject = Subject {
        name: spec::founding::admin(),
        private_key: PrivateKey([8; 32]),
    };
    let error = Client::connect(&node, subject).unwrap_err();
    let unreachable = transport::Error::Unreachable {
        peer,
        attempts: vec![(transport::Address::Udp(address), transport::Error::TimedOut)],
    };
    assert_eq!(
        error,
        Error::Session(program::Error::Transport(unreachable))
    );
}

/// A writer of the defaults writes values with the node's stamp, values with their own
/// stamp, and series with their stamps, in any order of places, and a reader gets each
/// frame in order once the close returns.
#[test]
fn a_writer_writes_values_and_series_that_a_reader_gets_in_order() {
    let (mut rig, node, subject) = planted();
    let read = reader(&node, &subject, "plant.*", 3);
    let client = Client::connect(&node, subject).unwrap();
    let channels = ["plant.b".parse().unwrap(), "plant.a".parse().unwrap()];
    let mut writer = client.open_writer(Config::new(channels.into())).unwrap();
    let names: Vec<String> =
        writer.places().iter().map(|p| p.name.to_string()).collect();
    assert_eq!(names, ["plant.time", "plant.a", "plant.b"]);
    let [time, a, b] = ["plant.time", "plant.a", "plant.b"].map(|n| place(&writer, n));
    let values = [(b, Value::I64(2)), (a, Value::I64(1))];
    assert_eq!(writer.write(&values), Ok(taken(0, 1)));
    let first = wait(&read);
    let stamp = first["plant.time"][0];
    assert_ne!(stamp, 0);
    let samples = |frame: &Samples| {
        let without = frame.iter().filter(|(name, _)| *name != "plant.time");
        without
            .map(|(name, values)| (name.clone(), values.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        samples(&first),
        [("plant.a".into(), vec![1]), ("plant.b".into(), vec![2])]
    );
    let given = Value::Stamp(Stamp::from_nanos(stamp + 5));
    assert_eq!(
        writer.write(&[(a, Value::I64(3)), (time, given)]),
        Ok(taken(1, 1))
    );
    let second = wait(&read);
    assert_eq!(second["plant.time"], [stamp + 5]);
    assert_eq!(samples(&second), [("plant.a".into(), vec![3])]);
    let stamps = [stamp + 10, stamp + 20].map(i64::to_le_bytes).concat();
    let counts = [4_i64, 5].map(i64::to_le_bytes).concat();
    let series = [(b, counts.as_slice()), (time, stamps.as_slice())];
    assert_eq!(writer.write_series(2, &series), Ok(taken(2, 2)));
    assert_eq!(writer.close(), Ok(()));
    let third = wait(&read);
    assert_eq!(third["plant.time"], [stamp + 10, stamp + 20]);
    assert_eq!(samples(&third), [("plant.b".into(), vec![4, 5])]);
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// A value of another type than its channel gives the exact error with nothing sent,
/// and the writer goes on.
#[test]
fn a_write_of_a_value_of_another_type_sends_nothing_and_the_writer_goes_on() {
    let (mut rig, node, subject) = planted();
    let read = reader(&node, &subject, "plant.a", 1);
    let client = Client::connect(&node, subject).unwrap();
    let channels = vec!["plant.a".parse().unwrap()];
    let mut writer = client.open_writer(Config::new(channels)).unwrap();
    let a = place(&writer, "plant.a");
    let error = writer.write(&[(a, Value::F64(1.5))]).unwrap_err();
    let value = Error::Value {
        place: a,
        scalar: Scalar::F64,
        data_type: Type::Scalar(Scalar::I64),
    };
    assert_eq!(error, value);
    assert_eq!(error.to_string(), format!("place {a} holds i64, not f64"));
    assert_eq!(writer.write(&[(a, Value::I64(7))]), Ok(taken(0, 1)));
    assert_eq!(wait(&read)["plant.a"], [7]);
    assert_eq!(writer.close(), Ok(()));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

#[test]
fn an_open_of_a_writer_of_an_unknown_name_gives_unknown_and_the_client_goes_on() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let channels = vec!["plant.missing".parse().unwrap()];
    let opened = client.open_writer(Config::new(channels)).map(drop);
    let unknown = Error::Stream(program::Error::Refused(Refusal::Unknown));
    assert_eq!(opened, Err(unknown));
    let channels = vec!["plant.a".parse().unwrap()];
    let writer = client.open_writer(Config::new(channels)).unwrap();
    assert_eq!(writer.close(), Ok(()));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// The node refuses a frame whose series is not `count` samples, and the writer goes
/// on.
#[test]
fn a_write_of_series_whose_bytes_are_not_count_samples_is_refused() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let channels = vec!["plant.a".parse().unwrap()];
    let mut writer = client.open_writer(Config::new(channels)).unwrap();
    let (time, a) = (place(&writer, "plant.time"), place(&writer, "plant.a"));
    let stamps = [1_i64, 2].map(i64::to_le_bytes).concat();
    let short = 4_i64.to_le_bytes();
    let written = writer.write_series(2, &[(time, &stamps), (a, &short)]);
    assert_eq!(written, Ok(Outcome::Refused(Refused::Codec)));
    assert_eq!(writer.write(&[(a, Value::I64(5))]), Ok(taken(0, 1)));
    assert_eq!(writer.close(), Ok(()));
    assert_eq!(client.close(), Ok(()));
    rig.stop();
}

/// The close of a client with a live writer returns, and each later call of the
/// writer gives `Closed`.
#[test]
fn a_close_of_a_client_with_a_live_writer_returns_and_ends_the_writer() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let channels = vec!["plant.a".parse().unwrap()];
    let mut writer = client.open_writer(Config::new(channels)).unwrap();
    let a = place(&writer, "plant.a");
    assert_eq!(writer.write(&[(a, Value::I64(1))]), Ok(taken(0, 1)));
    assert_eq!(client.close(), Ok(()));
    assert_eq!(writer.write(&[(a, Value::I64(2))]), Err(Error::Closed));
    assert_eq!(writer.write_series(1, &[]), Err(Error::Closed));
    assert_eq!(writer.close(), Err(Error::Closed));
    rig.stop();
}

/// A drop of a client does not wait, so a call of its writer gives `Closed` once the
/// client's thread ends the writer.
#[test]
fn a_call_of_a_writer_after_a_drop_of_its_client_gives_closed() {
    let (mut rig, node, subject) = planted();
    let client = Client::connect(&node, subject).unwrap();
    let channels = vec!["plant.a".parse().unwrap()];
    let mut writer = client.open_writer(Config::new(channels)).unwrap();
    let a = place(&writer, "plant.a");
    drop(client);
    rig.wait("a write gives closed", || {
        match writer.write(&[(a, Value::I64(1))]) {
            Err(Error::Closed) => Ok(()),
            other => Err(format!("{other:?}")),
        }
    });
    assert_eq!(writer.close(), Err(Error::Closed));
    rig.stop();
}
