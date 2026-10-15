//! A node on the real OS frees its port before it frees the lock of its data
//! directory.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::mpsc;

use env::files::Mode;
use transport::{Address, Class, Code};
use types::ed25519::PrivateKey;
use types::time::Span;

use crate::common::{PATIENCE, files, free, recv, start, start_node, wait};
use crate::rig::Rig;

/// The private key of the node.
const KEY: PrivateKey = PrivateKey([2; 32]);

/// A peer holds a session when the node stops. The probe takes the lock as soon as
/// the node frees it, and binds the node's port at once, with no retry.
#[test]
fn the_port_binds_at_once_when_the_lock_is_free() {
    let rig = Rig::new();
    let (shards, threads) = (os::shards().unwrap(), os::threads().unwrap());
    let io = Rc::new(RefCell::new(Vec::new()));
    let disk = files(&rig.dir, &threads, &io);
    run(&shards, "key", move |_| async move {
        let files = env::files::Files::new(disk);
        let own = types::node::Key::from_u128(1);
        node::create_key(&files, &os::entropy(), own, KEY)
            .await
            .unwrap();
    });
    let listen = free();
    let region = node::Region::Absent;
    let node = start_node(&rig, &shards, &threads, &io, listen, region, |disk| disk);
    // A task runs once the node takes sessions. The peer dials only then: a dial waits
    // 30 s, and a loaded host can take longer to serve.
    let (serves, serving) = mpsc::channel();
    node.spawn(move |_| {
        serves.send(()).unwrap();
        async {}
    });
    if let Err(error) = recv(&serving, PATIENCE) {
        node.stop();
        panic!("the node does not serve ({error}): {:?}", node.join());
    }
    let (opened, open) = mpsc::channel();
    let (closed, close) = mpsc::channel();
    let peer = start(&shards, "peer", move |tasks| async move {
        let (client, pool) = client(tasks);
        let session = client
            .dial(KEY.public(), &[Address::Udp(listen)])
            .await
            .expect("a session");
        // The node rejects a stream only once it has the session too.
        let (mut sender, mut receiver) = session.open(Class::Command).await.unwrap();
        sender.send(pool.copy(&[0]).unwrap()).await.unwrap();
        opened.send(receiver.recv().await.err()).unwrap();
        closed.send(session.closed().await).unwrap();
    });
    let rejected = transport::Error::Reset {
        code: Code(wire::header::REJECTED),
    };
    assert_eq!(wait(&open), Some(rejected));
    node.stop();
    let disk = files(&rig.dir, &threads, &io);
    let (bound, bind) = mpsc::channel();
    let probe = start(&shards, "probe", move |_| async move {
        let files = env::files::Files::new(disk);
        let lock = lock(&files).await;
        bound
            .send(transport::Port::bind(&os::net(), listen).map(drop))
            .unwrap();
        drop(lock);
    });
    assert_eq!(wait(&bind), Ok(()));
    let peer_closed = transport::Error::PeerClosed { code: Code(0) };
    assert_eq!(wait(&close), peer_closed);
    assert_eq!(node.join(), Ok(()));
    for shard in [peer, probe] {
        shard.join().unwrap();
    }
    for thread in io.take() {
        thread.join().unwrap();
    }
}

/// Takes the lock of the data directory as soon as no node holds it.
async fn lock(files: &env::files::Files) -> env::files::File {
    let clock = os::clock();
    loop {
        let mode = Mode::Create { len: 0 };
        match files.open(Path::new("lock"), mode).await {
            Err(env::files::Error::Busy { .. }) => {
                clock.sleep(Span::from_nanos(1_000)).await;
            }
            opened => return opened.expect("the lock opens"),
        }
    }
}

/// Runs shard `name` on `main` until it ends.
fn run<F: Future<Output = ()> + 'static>(
    shards: &env::shards::Shards,
    name: &str,
    main: impl FnOnce(env::tasks::Tasks) -> F + Send + 'static,
) {
    start(shards, name, main).join().unwrap();
}

/// A program's transport on a free port of `[::]`, and its pool.
fn client(tasks: env::tasks::Tasks) -> (transport::Client, Rc<block::Pool>) {
    let config = block::Config::new(1 << 20).expect("the budget fits");
    let memory = block::Heap::new(config.reservation());
    let pool = Rc::new(block::Pool::new(config, memory));
    let config = transport::client::Config {
        net: os::net(),
        clock: os::clock(),
        entropy: os::entropy(),
        tasks,
        pool: Rc::clone(&pool),
    };
    (transport::Client::new(config).unwrap(), pool)
}
