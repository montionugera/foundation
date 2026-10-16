//! Asserts that `secret` erases a key or a value before it frees the memory that
//! held it. The scan covers every thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::hint::black_box;
use std::path::Path;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use aws_lc_rs::agreement::{self, PrivateKey, UnparsedPublicKey, X25519};
use env::files::Mode;
use secret::seal::{Opener, seal};
use secret::store::{Sealed, Store};
use secret::{Error, Resolver, Source, Value};
use types::name::Name;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const PLAIN: [u8; 32] = [0x5a; 32];

fn main() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let entropy = sim.node(sim::node::Config::default()).entropy();
    let name: Name = "site.secrets.token".parse().expect("a valid name");

    let opener = Opener::generate(&entropy);
    let key = *opener.expose();
    let ((), found) = ALLOCATOR.freed_holding(&key, || drop(black_box(opener)));
    assert_eq!(found, 0, "a dropped opener erases its key");

    let ((), found) =
        ALLOCATOR.freed_holding(&PLAIN, || drop(black_box(Value::new(PLAIN.to_vec()))));
    assert_eq!(found, 0, "a dropped value erases its bytes");

    let opener = Opener::generate(&entropy);
    let key = *opener.expose();
    let to = opener.public();
    let mut store = Sealed::new(opener);
    let value = Value::new(PLAIN.to_vec());
    let (sealed, found) =
        ALLOCATOR.freed_holding(&PLAIN, || seal(&to, &name, 1, &value, &entropy));
    assert_eq!(found, 0, "seal leaves the value in no freed block");
    let again = seal(&to, &name, 2, &value, &entropy);
    let last = seal(&to, &name, 3, &value, &entropy);
    let shared = shared_secret(&key, &last);
    drop(value);

    let mut round = |version, sealed| {
        store
            .put(name.clone(), version, sealed)
            .expect("the value opens");
        let mut request = pin!(store.get(&name));
        let Poll::Ready(answer) = request
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        else {
            panic!("the sealed store answers on the first poll");
        };
        let value = answer.expect("the store has the value");
        assert_eq!(value.expose(), PLAIN, "the store opens the value put");
    };
    let ((), found) = ALLOCATOR.freed_holding(&PLAIN, || round(1, sealed));
    assert_eq!(found, 0, "put and get leave the value in no freed block");
    let ((), found) = ALLOCATOR.freed_holding(&key, || round(2, again));
    assert_eq!(found, 0, "put and get leave the key in no freed block");
    let ((), found) = ALLOCATOR.freed_holding(&shared, || round(3, last));
    assert_eq!(
        found, 0,
        "put and get leave the shared secret in no freed block"
    );

    let (refused, found) = refused();
    assert_eq!(found, 0, "a refused get leaves the value in no freed block");
    let exposed = "the group or other users can read the file \
                   /data/secrets/site.token. Make it readable only by the user that \
                   runs the node: chmod 600 /data/secrets/site.token";
    assert_eq!(
        refused,
        Err(Error::Unavailable {
            detail: exposed.into()
        }),
        "the store refuses a file that others can read"
    );
}

/// A get of the file store, on the sim disk, of a file that holds `PLAIN` and that
/// the group or other users can read. Gives what it gave and the count of freed
/// blocks that hold `PLAIN` during the get.
fn refused() -> (Result<(), Error>, u64) {
    let mut sim = sim::Sim::new(sim::Config::default());
    let node = sim.node(sim::node::Config::default());
    let path = Path::new("secrets/site.token");
    let put = sim.run_on(&node, move |node, _| async move {
        let files = node.files();
        files
            .create_dir(Path::new("secrets"))
            .await
            .expect("makes it");
        let len = u64::try_from(PLAIN.len()).expect("fits");
        let file = files.open(path, Mode::Create { len }).await;
        let file = file.expect("makes it");
        let pool = block::Pool::heap(block::Config::new(1 << 18).expect("fits"));
        let block = pool.copy(&PLAIN).expect("holds it");
        file.write_at(0, &[block]).await.expect("writes it");
        file.close().await;
        node.set_file_exposed(path, true);
    });
    put.expect("the run ends");
    ALLOCATOR.freed_holding(&PLAIN, || {
        let get = sim.run_on(&node, |node, _| async move {
            let resolver =
                Resolver::new(Arc::default(), node.files(), Path::new("secrets"));
            let source = Source::Secret("site.token".parse().expect("a valid name"));
            resolver.get(&source).await.map(drop)
        });
        get.expect("the run ends")
    })
}

/// The X25519 shared secret of the node key `own` and the encapsulated key that
/// starts `sealed`.
fn shared_secret(own: &[u8; 32], sealed: &[u8]) -> Vec<u8> {
    let own = PrivateKey::from_private_key(&X25519, own).expect("a valid key");
    let peer = UnparsedPublicKey::new(&X25519, &sealed[..32]);
    agreement::agree(&own, peer, (), |dh| Ok(dh.to_vec())).expect("the keys agree")
}
