use std::collections::BTreeMap;
use std::rc::Rc;

use ::mesh::Member;
use ::mesh::card::addresses::Addresses;
use ::mesh::card::{self, Card};
use ::mesh::region::Founding;
use ::mesh::status::Status;
use transport::Address;
use types::name::Prefix;

use super::*;
use crate::admin::{FILE, LEN};

/// The file that `sector::publish` renames to [`FILE`].
const NEW: &str = "admin.key.new";
/// The founding file of the mesh directory.
const FOUNDING: &str = "mesh/founding";

/// Starts the node on `host`, which [`keyed`] made, with the region that its data
/// directory holds.
fn start(host: &sim::node::Node) -> Node {
    Node::start(Config {
        region: Region::Stored,
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    })
}

/// Starts the node on `host` with [`start`], runs `sim` for a second, then stops it
/// and gives the error of its join.
fn start_and_stop(sim: &mut sim::Sim, host: &sim::node::Node) -> Result<(), Error> {
    let node = start(host);
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    node.join()
}

/// The bytes of the file `path` on `host`, or `None` when it is not there.
fn bytes(sim: &mut sim::Sim, host: &sim::node::Node, path: &str) -> Option<Vec<u8>> {
    let path = PathBuf::from(path);
    sim.run_on(host, move |host, _| async move {
        let file = match host.files().open(&path, env::files::Mode::Read).await {
            Err(env::files::Error::NotFound { .. }) => return None,
            file => file.expect("opens"),
        };
        let len = usize::try_from(file.len()).unwrap();
        if len == 0 {
            return Some(Vec::new());
        }
        let pool =
            block::Pool::heap(block::Config::new(1 << 16).expect("the budget fits"));
        let into = pool.alloc(len).expect("a block");
        Some(file.read_at(0, into).await.expect("reads").to_vec())
    })
    .expect("the run ends")
}

/// Writes `bytes` to a new file `path` on `host`, and makes it durable.
fn write(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
    path: &'static str,
    bytes: Vec<u8>,
) {
    sim.run_on(host, move |host, _| async move {
        let files = host.files();
        let mode = env::files::Mode::Create {
            len: bytes.len() as u64,
        };
        let file = files.open(Path::new(path), mode).await.expect("opens");
        if !bytes.is_empty() {
            let pool =
                block::Pool::heap(block::Config::new(4096).expect("the budget fits"));
            let block = pool.copy(&bytes).expect("a block");
            file.write_at(0, &[block]).await.expect("writes");
        }
        file.sync().await.expect("syncs");
        files.sync_dir(Path::new("")).await.expect("syncs");
    })
    .expect("the run ends");
}

/// The founding in the mesh directory of `host`.
fn stored(sim: &mut sim::Sim, host: &sim::node::Node) -> Option<Founding> {
    sim.run_on(host, |host, _| async move {
        let pool = Rc::new(block::Pool::heap(
            block::Config::new(1 << 20).expect("the budget fits"),
        ));
        let (files, dir) = (host.files(), crate::directory::mesh());
        let stored = ::mesh::founding(&files, &dir, pool).await;
        stored.expect("the founding reads")
    })
    .expect("the run ends")
}

/// The private key in `admin.key` on `host`, whose tag and checksum it checks.
fn admin(sim: &mut sim::Sim, host: &sim::node::Node) -> PrivateKey {
    let bytes = bytes(sim, host, FILE).expect("the file");
    assert_eq!(bytes.len(), LEN);
    assert_eq!(&bytes[..18], b"foundation/admin/1");
    let crc = crc32c::crc32c(&bytes[..50]).to_le_bytes();
    assert_eq!(bytes[50..], crc, "little-endian");
    PrivateKey(bytes[18..50].try_into().unwrap())
}

/// The root region that the node [`OWN`] on `host` founds alone, as `edge`, whose spec
/// holds the first admin `admin`.
fn root(host: &sim::node::Node, admin: &PrivateKey) -> Founding {
    let card = Card {
        name: "edge".parse().unwrap(),
        public_key: KEY.public(),
        seal_key: Opener::from_bytes(SEAL).public(),
        addresses: Addresses::new(vec![Address::Udp(listen(host))]).unwrap(),
        version: 1,
    };
    let member = Member {
        card: card::Signed::sign(OWN, card, &KEY),
        admission: [0; 64],
        ephemeral: None,
        status: Status::new(BTreeMap::new()).unwrap(),
    };
    Founding {
        prefix: Prefix::ROOT,
        members: vec![member],
        voters: [OWN].into(),
        definitions: spec::founding::create(admin.public()),
        homes: BTreeMap::new(),
    }
}

#[test]
fn the_first_start_founds_the_root_region_alone() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let admin = admin(&mut sim, &host);
    assert_ne!(admin.0, KEY.0, "the admin is not the node");
    assert_eq!(stored(&mut sim, &host), Some(root(&host, &admin)));
}

/// Two first starts on two hosts make two admin keys from the entropy.
#[test]
fn a_first_start_makes_the_admin_key_from_the_entropy() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let hosts = [keyed(&mut sim, 2), keyed(&mut sim, 2)];
    let admins = hosts.map(|host| {
        assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
        admin(&mut sim, &host)
    });
    assert_ne!(admins[0].0, admins[1].0);
}

/// A later start opens the region of the first start, also on another port, and
/// keeps `admin.key` and `node.key`. A task of `operate` runs on that region.
#[test]
fn a_later_start_opens_the_region_of_the_first_start() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let founding = bytes(&mut sim, &host, FOUNDING).expect("the founding");
    let admin = bytes(&mut sim, &host, FILE).expect("the admin key");
    let key = bytes(&mut sim, &host, identity::FILE).expect("the node key");
    let node = Node::start(Config {
        region: Region::Stored,
        listen: SocketAddr::new(host.addresses()[0], PORT + 1),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    let names = Arc::new(Mutex::new(None));
    let out = Arc::clone(&names);
    node.operate(move |ops| async move {
        *out.lock().unwrap() = Some(ops.mesh().names());
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    let edge = BTreeSet::from(["edge".parse().unwrap()]);
    assert_eq!(names.lock().unwrap().take(), Some(edge));
    assert_eq!(bytes(&mut sim, &host, FOUNDING), Some(founding));
    assert_eq!(bytes(&mut sim, &host, FILE), Some(admin));
    assert_eq!(bytes(&mut sim, &host, identity::FILE), Some(key));
}

/// A first start founds the region with the admin key in its data directory, and
/// keeps the file. The input `admin-valid` of the fuzz corpus is that file, and
/// `admin-valid-body` with its CRC32C is `admin-valid`.
#[test]
fn a_first_start_founds_with_the_admin_key_of_the_corpus() {
    let valid: &[u8; LEN] =
        include_bytes!("../../../../oracles/fuzz/node_identity/admin-valid");
    let body =
        include_bytes!("../../../../oracles/fuzz/node_identity/admin-valid-body");
    assert_eq!(crate::sector::summed(body).as_ref(), Some(valid));
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    write(&mut sim, &host, FILE, valid.to_vec());
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let founded = root(&host, &PrivateKey([7; 32]));
    assert_eq!(stored(&mut sim, &host), Some(founded));
    assert_eq!(bytes(&mut sim, &host, FILE), Some(valid.to_vec()));
}

/// A start removes the file `admin.key.new` that a crash left, of any length, and
/// makes `admin.key`.
#[test]
fn a_first_start_removes_a_new_admin_key_that_a_crash_left() {
    for bytes in [Vec::new(), vec![7; LEN], vec![7; 3]] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        write(&mut sim, &host, NEW, bytes);
        assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
        let admin = admin(&mut sim, &host);
        assert_eq!(stored(&mut sim, &host), Some(root(&host, &admin)));
        assert!(!listed(&mut sim, &host, "").contains(&PathBuf::from(NEW)));
    }
}

/// A start whose data directory holds a region makes no `admin.key`.
#[test]
fn a_start_with_a_region_makes_no_admin_key() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let founding = stored(&mut sim, &host);
    sim.run_on(&host, |host, _| async move {
        let files = host.files();
        files.remove(Path::new(FILE)).await.expect("removes");
        files.sync_dir(Path::new("")).await.expect("syncs");
    })
    .expect("the run ends");
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    assert_eq!(bytes(&mut sim, &host, FILE), None);
    assert_eq!(stored(&mut sim, &host), founding);
}

/// A power cut at each time of the first start leaves no `admin.key` or a whole one,
/// and a data directory whose next start opens or founds a region with the first
/// admin of that file. Some cuts come after `admin.key` is durable and before the
/// founding is.
#[test]
fn a_power_cut_during_the_first_start_keeps_the_admin_of_the_region() {
    let mut between = 0;
    for seed in 0..4 {
        for after in (0..).step_by(25_000).map(Span::from_nanos) {
            let mut sim = sim::Sim::new(sim::Config {
                seed,
                ..sim::Config::default()
            });
            let host = keyed(&mut sim, 2);
            let node = start(&host);
            assert_eq!(sim.run_for(after), Ok(()), "{seed} {after:?}");
            sim.crash(&host, sim::Crash::Power);
            drop(node);
            let kept = stored(&mut sim, &host);
            let written = bytes(&mut sim, &host, FILE).map(|_| admin(&mut sim, &host));
            between += usize::from(written.is_some() && kept.is_none());
            assert_eq!(start_and_stop(&mut sim, &host), Ok(()), "{seed} {after:?}");
            let admin = admin(&mut sim, &host);
            if let Some(written) = written {
                assert_eq!(written.0, admin.0, "{seed} {after:?}");
            }
            let founded = Some(root(&host, &admin));
            assert_eq!(stored(&mut sim, &host), founded, "{seed} {after:?}");
            if kept.is_some() {
                assert_eq!(kept, founded, "{seed} {after:?}");
                break;
            }
        }
    }
    assert!(between > 0, "no cut came between the two files");
}

/// An `admin.key` of another length, also one with no bytes, or of another tag or
/// checksum, also 54 zero bytes, stops the node, which keeps the file and founds no
/// region.
#[test]
fn an_admin_key_that_is_not_valid_stops_the_node() {
    let valid: [u8; LEN] =
        *include_bytes!("../../../../oracles/fuzz/node_identity/admin-valid");
    // Each tag byte with one bit changed, and the tag of the next form.
    let edits = (0..18).map(|i| (i, valid[i] ^ 1)).chain([(17, b'2')]);
    let tags = edits.map(|(i, byte)| {
        let mut tag = valid;
        tag[i] = byte;
        crate::sector::checksum(&mut tag);
        tag.to_vec()
    });
    let mut changed = valid;
    changed[30] ^= 1;
    let short = valid[..LEN - 1].to_vec();
    let long = [valid.as_slice(), &[0]].concat();
    let unwritten = [Vec::new(), vec![0; LEN]];
    let lengths = [short, long, changed.to_vec()];
    for foreign in unwritten.into_iter().chain(lengths).chain(tags) {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        write(&mut sim, &host, FILE, foreign.clone());
        let node = start(&host);
        assert_eq!(sim.run(), Ok(()));
        assert_eq!(node.join(), Err(Error::Admin));
        assert_eq!(bytes(&mut sim, &host, FILE), Some(foreign));
        assert_eq!(stored(&mut sim, &host), None);
    }
    assert_eq!(
        Error::Admin.to_string(),
        "the file `admin.key` in the data directory is not an admin key; restore it \
         from a backup, or remove it to make a new admin"
    );
}

/// A failed file call on `admin.key`, or on the `admin.key.new` that makes it, stops
/// the node before it founds a region, and the next start founds one. A failed read
/// keeps the key that the file holds.
#[test]
fn a_failed_file_call_on_the_admin_key_stops_the_node() {
    use env::files::Operation::{Open, ReadAt, Remove, Rename, Sync, WriteAt};
    let valid: [u8; LEN] =
        *include_bytes!("../../../../oracles/fuzz/node_identity/admin-valid");
    for (path, operation) in [
        (NEW, Remove),
        (NEW, Open),
        (NEW, WriteAt),
        (NEW, Sync),
        (NEW, Rename),
        (FILE, Open),
        (FILE, ReadAt),
    ] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        if path == FILE {
            write(&mut sim, &host, FILE, valid.to_vec());
        }
        host.fail_file(Path::new(path), operation);
        let error = env::files::Error::Io {
            path: PathBuf::from(path),
            operation,
            code: 5,
        };
        let failed = start_and_stop(&mut sim, &host);
        assert_eq!(failed, Err(Error::Directory(error)), "{operation:?} {path}");
        assert_eq!(stored(&mut sim, &host), None, "{operation:?} {path}");
        assert_eq!(
            start_and_stop(&mut sim, &host),
            Ok(()),
            "{operation:?} {path}"
        );
        let admin = admin(&mut sim, &host);
        if path == FILE {
            assert_eq!(admin.0, [7; 32], "{operation:?} {path}");
        }
        let founded = Some(root(&host, &admin));
        assert_eq!(stored(&mut sim, &host), founded, "{operation:?} {path}");
    }
}

/// A founding that does not read stops the node before it makes `admin.key`.
#[test]
fn a_founding_that_does_not_read_stops_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    let operation = env::files::Operation::Open;
    host.fail_file(Path::new(FOUNDING), operation);
    let error = ::mesh::Error::Files(env::files::Error::Io {
        path: PathBuf::from(FOUNDING),
        operation,
        code: 5,
    });
    assert_eq!(start_and_stop(&mut sim, &host), Err(Error::Mesh(error)));
    assert_eq!(bytes(&mut sim, &host, FILE), None);
}
