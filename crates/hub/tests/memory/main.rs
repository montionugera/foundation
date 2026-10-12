//! A request that the hub refuses for its length allocates no body, the heap of a hub
//! does not grow over many opens and closes of a named complete reader, and a read
//! that waits for its home holds no places. This
//! binary has no test harness: the count covers each thread, and a harness allocates
//! on its own thread at any time.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

#[path = "../common/agent.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod agent;
#[path = "../common/mod.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod common;
#[path = "../common/net.rs"]
mod net;
#[path = "../common/node.rs"]
mod node;
#[path = "../common/region.rs"]
mod region;
#[path = "../common/sessions.rs"]
#[expect(dead_code, reason = "the binary uses only some of the helpers")]
mod sessions;

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use agent::{Agent, SUBJECT, name, reset_with};
use hub::reader::{self, Mode};
use hub::serve;
use sessions::{closed_after, rules, serve_each, sessions};
use transport::{Class, Code};
use types::channel;
use types::name::Selector;
use types::time::Span;
use wire::hub::client::{self, BODY_BYTES_MAX, FromNode, OPEN_BYTES_MAX, Place};
use wire::hub::{BUSY, FromHome, Open, keys};

#[global_allocator]
static ALLOCATOR: counting::Bytes = counting::Bytes::new();

/// The opens after which the heap is read the first time.
const FEW: usize = 10;
/// The opens after which the heap is read the last time.
const MANY: usize = 1000;

fn main() {
    refuses_a_request_over_the_share_before_it_allocates_the_body();
    holds_the_same_bytes_over_many_opens_of_a_named_reader();
    holds_no_more_bytes_for_an_open_that_names_one_channel_many_times();
    holds_no_places_while_a_read_waits_for_its_home();
    holds_no_more_for_a_read_of_one_channel_whose_open_is_at_the_cap();
    holds_no_places_once_a_read_opens();
}

/// Patterns `x` that fill the open of a read of `value` to [`OPEN_BYTES_MAX`]: one
/// mode byte, `value` (2 + 5), and 3 bytes for each `x`.
const PADS: usize = (65_536 - 8) / 3;

/// The mesh names no home of `time`, so each read waits for it. A read of `value`
/// whose open is at the cap holds, while it waits, at most its open more than a read
/// of `value`, plus a tenth of the places of [`WIDE`] channels, the bound of
/// `holds_no_places_while_a_read_waits_for_its_home`.
fn holds_no_more_for_a_read_of_one_channel_whose_open_is_at_the_cap() {
    let long = "x".repeat(240);
    let places: usize = (0..WIDE)
        .map(|n| size_of::<Place>() + format!("w.{long}.{n}").len())
        .sum();
    let measured = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&measured);
    let home = |node: sim::node::Node, tasks: env::tasks::Tasks| async move {
        let transport =
            net::transport(&node, &tasks, &net::own_pool(), net::HOME, 1 << 16);
        let transport = Rc::new(transport);
        let region =
            region::open(&node, &tasks, Rc::clone(&transport), Vec::new()).await;
        let hub = common::hub_in(&node, tasks.clone(), Some(region), &[]).await;
        hub.set_rules(rules());
        let session = transport.accept().await.expect("a session");
        let link = hub.link(session.clone());
        let (served, accepted) = (Arc::default(), Rc::new(Cell::new(0)));
        let clock = node.clock();
        tasks.spawn(serve_each(
            &session, &link, &tasks, &clock, &served, &accepted,
        ));
        drop(session.closed().await);
    };
    let program = move |node: sim::node::Node, tasks, at| async move {
        let mut agent = Agent::dial(&node, tasks, at, name(SUBJECT)).await;
        agent.admit().await;
        agent.sleep(common::SETTLE).await;
        let before = ALLOCATOR.held();
        let _one = agent.read(client::Mode::Complete, &["value"]).await;
        agent.sleep(common::SETTLE).await;
        let one = ALLOCATOR.held();
        let mut select = vec!["value"];
        select.extend(std::iter::repeat_n("x", PADS));
        let _padded = agent.read(client::Mode::Complete, &select).await;
        drop(select);
        agent.sleep(common::SETTLE).await;
        let padded = ALLOCATOR.held();
        *kept.lock().expect("not poisoned") = Some((one - before, padded - one));
        agent.session.close(Code(0));
        node.clock().sleep(Span::MILLISECOND).await;
    };
    sessions::run_program(3, home, program);
    let (one, padded) = measured
        .lock()
        .expect("not poisoned")
        .take()
        .expect("the program measured");
    let open = usize::try_from(OPEN_BYTES_MAX).expect("fits");
    assert!(
        padded < one + open + places / 10,
        "a read of value holds {one} bytes, and one of value with an open at the cap \
         holds {padded}: the open is {open} bytes, and a tenth of the places of {WIDE} \
         channels is {}",
        places / 10
    );
}

/// Keys of `value` that the peer sends in each message.
const KEYS: usize = 1024;
/// Messages of keys that the peer sends.
const MESSAGES: usize = 1280;
/// The most bytes that the home may hold for the keys of one open of two channels.
const OPEN_MAX: usize = 4 << 20;

/// A peer node opens a latest session of `u32::MAX` channels, and sends `value` up
/// to 1.3 million times. The home stops the stream with `MALFORMED` at the second
/// `value`, so it holds no more bytes after the keys than before them.
fn holds_no_more_bytes_for_an_open_that_names_one_channel_many_times() {
    let measured = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&measured);
    let home = |node: sim::node::Node, tasks: env::tasks::Tasks| async move {
        let (hub, _) = common::hub(&node, tasks.clone()).await;
        let transport =
            net::transport(&node, &tasks, &net::own_pool(), net::HOME, 1 << 16);
        let (session, incoming) = net::accept(&transport).await;
        let link = hub.link(session.clone());
        drop(link.serve(incoming).await);
        drop(session.closed().await);
    };
    let peer = move |node: sim::node::Node, tasks: env::tasks::Tasks, at| async move {
        let transport =
            net::transport(&node, &tasks, &net::own_pool(), net::PEER, 1 << 16);
        let config = block::Config::new(8 << 20).expect("the budget fits");
        let pool =
            block::Pool::new(config.clone(), block::Heap::new(config.reservation()));
        let session = transport
            .dial(net::public_key(&net::HOME), &[at])
            .await
            .expect("dials");
        let (mut sender, _receiver) = session.open(Class::Latest).await.expect("opens");
        let mut send = async |bytes: &[u8]| {
            let mut block = pool.alloc(bytes.len()).expect("the pool has room");
            block.copy_from_slice(bytes);
            sender.send(block.freeze()).await
        };
        send(&wire::header::encode(wire::Protocol::Hub))
            .await
            .expect("sends the header");
        let open = Open {
            mode: wire::hub::Mode::Latest,
            channels: u32::MAX,
        };
        let mut out = vec![0; open.encoded_len()];
        open.encode(&mut out);
        send(&out).await.expect("sends the open");
        let mut run = vec![0; KEYS * keys::LEN];
        keys::encode(&[channel::Key::from_u128(2); KEYS], &mut run);
        send(&run).await.expect("sends keys");
        node.clock().sleep(common::SETTLE).await;
        let before = ALLOCATOR.held();
        let mut failed = None;
        for _ in 1..MESSAGES {
            if let Err(error) = send(&run).await {
                failed = Some(error);
                break;
            }
        }
        let code = Code(wire::header::MALFORMED);
        let stopped = transport::Error::Stopped { code };
        assert_eq!(failed, Some(stopped), "the home stops the stream");
        node.clock().sleep(common::SETTLE).await;
        *kept.lock().expect("not poisoned") = Some((before, ALLOCATOR.held()));
        session.close(Code(0));
        node.clock().sleep(Span::MILLISECOND).await;
    };
    sessions::run_program(0, home, peer);
    let (before, after) = measured
        .lock()
        .expect("not poisoned")
        .take()
        .expect("the program measured");
    assert!(
        after.saturating_sub(before) < OPEN_MAX,
        "the keys of one open of two channels raised the bytes held by {} from {before}",
        after - before,
    );
}

/// While a request of 1 byte is open, a request of `BODY_BYTES_MAX` of the same
/// subject is over the share, and stops with `BUSY` before the hub allocates its body.
fn refuses_a_request_over_the_share_before_it_allocates_the_body() {
    let measured = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&measured);
    let open = async |node: &sim::node::Node, tasks: &env::tasks::Tasks| {
        (common::hub(node, tasks.clone()).await.0, ())
    };
    let served = sessions(
        1,
        [SUBJECT, SUBJECT],
        open,
        |[mut first, mut second]| async move {
            first.admit().await;
            second.admit().await;
            let _held = first.unfinished(1, &[]).await;
            first.sleep(Span::MILLISECOND).await;
            ALLOCATOR.reset_peak();
            let held = ALLOCATOR.held();
            let mut busy = second.unfinished(BODY_BYTES_MAX, &[]).await;
            assert_eq!(busy.recv().await, reset_with(BUSY), "the hub refuses it");
            *kept.lock().expect("not poisoned") = Some((held, ALLOCATOR.peak()));
        },
    );
    let refused = serve::Error::Share {
        subject: name(SUBJECT),
        length: BODY_BYTES_MAX,
        held: 1,
    };
    assert_eq!(
        served,
        closed_after(vec![Err(refused)], 2, 1),
        "refused by the share"
    );
    let (held, peak) = measured
        .lock()
        .expect("not poisoned")
        .take()
        .expect("the program measured");
    let body = usize::try_from(BODY_BYTES_MAX).expect("16 MiB is a usize");
    assert!(
        peak - held < body,
        "the refusal raised the bytes held by {} from {held}, a body of {body}",
        peak - held,
    );
}

/// After [`FEW`] opens and drops of a named complete reader, [`MANY`] leave the heap
/// at the same bytes. The sim runs on one thread, so the count is exact.
fn holds_the_same_bytes_over_many_opens_of_a_named_reader() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    sim.run_on(&node, |node, tasks| async move {
        let (hub, _) = common::hub(&node, tasks).await;
        let config = || reader::Config {
            select: Selector::new(["value"]).expect("a selector"),
            mode: Mode::Complete,
            subject: common::name("a"),
            name: Some(common::name("r")),
            hold: Span::SECOND,
        };
        let mut few = 0;
        for n in 1..=MANY {
            drop(hub.reader(config()).await.expect("opens"));
            if n == FEW {
                few = ALLOCATOR.held();
            }
        }
        assert_eq!(ALLOCATOR.held(), few, "bytes held after {MANY} opens");
    })
    .expect("the run ends");
}

/// The channels that the wide read of
/// [`holds_no_places_while_a_read_waits_for_its_home`] matches.
const WIDE: usize = 10_000;

/// The mesh names no home of `time`, so each read waits for it. A read of [`WIDE`]
/// channels with long names holds about as many bytes while it waits as a read of one
/// channel: at most a tenth of the bytes of its places more.
fn holds_no_places_while_a_read_waits_for_its_home() {
    let long = "x".repeat(240);
    let wide: Vec<_> = (0..WIDE).map(|n| name(&format!("w.{long}.{n}"))).collect();
    let places: usize = wide
        .iter()
        .map(|name| size_of::<Place>() + name.as_str().len())
        .sum();
    let measured = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&measured);
    let home = |node: sim::node::Node, tasks: env::tasks::Tasks| async move {
        let transport =
            net::transport(&node, &tasks, &net::own_pool(), net::HOME, 1 << 16);
        let transport = Rc::new(transport);
        let region =
            region::open(&node, &tasks, Rc::clone(&transport), Vec::new()).await;
        let hub = common::hub_in(&node, tasks.clone(), Some(region), &wide).await;
        hub.set_rules(rules());
        let session = transport.accept().await.expect("a session");
        let link = hub.link(session.clone());
        let (served, accepted) = (Arc::default(), Rc::new(Cell::new(0)));
        let clock = node.clock();
        tasks.spawn(serve_each(
            &session, &link, &tasks, &clock, &served, &accepted,
        ));
        drop(session.closed().await);
    };
    let program = move |node: sim::node::Node, tasks, at| async move {
        let mut agent = Agent::dial(&node, tasks, at, name(SUBJECT)).await;
        agent.admit().await;
        agent.sleep(common::SETTLE).await;
        let before = ALLOCATOR.held();
        let _one = agent.read(client::Mode::Complete, &["value"]).await;
        agent.sleep(common::SETTLE).await;
        let one = ALLOCATOR.held();
        let _wide = agent.read(client::Mode::Complete, &["w.**"]).await;
        agent.sleep(common::SETTLE).await;
        let wide = ALLOCATOR.held();
        *kept.lock().expect("not poisoned") = Some((one - before, wide - one));
        agent.session.close(Code(0));
        node.clock().sleep(Span::MILLISECOND).await;
    };
    sessions::run_program(2, home, program);
    let (one, wide) = measured
        .lock()
        .expect("not poisoned")
        .take()
        .expect("the program measured");
    assert!(
        wide < one + places / 10,
        "a read of one channel holds {one} bytes, and one of {WIDE} holds {wide}: its \
         places are {places} bytes"
    );
}

/// A read of [`WIDE`] channels with long names holds, once it opens, about as many
/// bytes as a read of [`WIDE`] channels with short names: at most a tenth of the
/// bytes by which their names differ.
fn holds_no_places_once_a_read_opens() {
    let long = "x".repeat(240);
    let longs = (0..WIDE).map(|n| name(&format!("l.{long}.{n}")));
    let shorts = (0..WIDE).map(|n| name(&format!("s.{n}")));
    let more: Vec<_> = longs.chain(shorts).collect();
    let (longs, shorts) = more.split_at(WIDE);
    let differ: usize = longs
        .iter()
        .zip(shorts)
        .map(|(long, short)| long.as_str().len() - short.as_str().len())
        .sum();
    let measured = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&measured);
    let home = |node: sim::node::Node, tasks: env::tasks::Tasks| async move {
        let transport =
            net::transport(&node, &tasks, &net::own_pool(), net::HOME, 1 << 16);
        let hub = common::hub_in(&node, tasks.clone(), None, &more).await;
        hub.set_rules(rules());
        let session = transport.accept().await.expect("a session");
        let link = hub.link(session.clone());
        let (served, accepted) = (Arc::default(), Rc::new(Cell::new(0)));
        let clock = node.clock();
        tasks.spawn(serve_each(
            &session, &link, &tasks, &clock, &served, &accepted,
        ));
        drop(session.closed().await);
    };
    let program = move |node: sim::node::Node, tasks, at| async move {
        let mut agent = Agent::dial(&node, tasks, at, name(SUBJECT)).await;
        agent.admit().await;
        // The first read of each set grows the pools and the state of its channels.
        let _first = opened(&agent, "l.**").await;
        let _second = opened(&agent, "s.**").await;
        agent.sleep(common::SETTLE).await;
        let before = ALLOCATOR.held();
        let _long = opened(&agent, "l.**").await;
        agent.sleep(common::SETTLE).await;
        let long = ALLOCATOR.held();
        let _short = opened(&agent, "s.**").await;
        agent.sleep(common::SETTLE).await;
        let short = ALLOCATOR.held();
        *kept.lock().expect("not poisoned") = Some((long - before, short - long));
        agent.session.close(Code(0));
        node.clock().sleep(Span::MILLISECOND).await;
    };
    sessions::run_program(4, home, program);
    let (long, short) = measured
        .lock()
        .expect("not poisoned")
        .take()
        .expect("the program measured");
    assert!(
        long.abs_diff(short) < differ / 10,
        "a read of {WIDE} channels with long names holds {long} bytes, and one with \
         short names holds {short}: their names differ by {differ} bytes"
    );
}

/// A complete read of `select` by `agent`, after the program got its places, their
/// run, and `Opened`. The program holds nothing of the places.
async fn opened(agent: &Agent, select: &str) -> agent::Stream {
    let mut stream = agent.read(client::Mode::Complete, &[select]).await;
    let mut start = client::Reader::new(client::Mode::Complete);
    let mut next = async || {
        let message = stream.recv().await.expect("receives");
        message.expect("a message before the finish")
    };
    let Ok(FromNode::Places(_)) = start.decode(&next().await) else {
        panic!("the places come first");
    };
    let mut reader = loop {
        let Ok(FromNode::Run { home, .. }) = start.decode(&next().await) else {
            panic!("a message of the run of places");
        };
        if let Some(home) = home {
            break home;
        }
    };
    assert!(
        matches!(reader.decode(&next().await), Ok(FromHome::Opened)),
        "then `Opened`"
    );
    stream
}
