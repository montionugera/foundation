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

/// Removes the file `path` on `host` and makes the removal durable.
fn remove(sim: &mut sim::Sim, host: &sim::node::Node, path: &'static str) {
    sim.run_on(host, move |host, _| async move {
        let files = host.files();
        files.remove(Path::new(path)).await.expect("removes");
        files.sync_dir(Path::new("")).await.expect("syncs");
    })
    .expect("the run ends");
}

/// Removes the file `name` on `host`, makes the removal durable, and gives the bytes
/// that it held.
fn remove_name(sim: &mut sim::Sim, host: &sim::node::Node) -> Vec<u8> {
    let held = kept(sim, host).expect("the file");
    remove(sim, host, crate::name::FILE);
    held
}

/// The name that [`crate::name`] gives on `host` with no name given.
#[expect(
    clippy::unwrap_in_result,
    reason = "a test helper panics on a run that does not end"
)]
fn named(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
) -> Result<types::name::Name, Error> {
    let named = sim.run_on(host, |host, _| async move {
        crate::name(&host.files(), None).await
    });
    named.expect("the run ends")
}

/// The bytes of the file `name` on `host`, or `None` when it is not there.
fn kept(sim: &mut sim::Sim, host: &sim::node::Node) -> Option<Vec<u8>> {
    bytes(sim, host, crate::name::FILE)
}

#[test]
fn a_start_with_another_name_after_the_name_file_is_gone() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let edge = kept(&mut sim, &host).expect("the file");
    sim.run_on(&host, |host, _| async move {
        let files = host.files();
        files.remove(Path::new("name")).await.expect("removes");
        files.sync_dir(Path::new("")).await.expect("syncs");
    })
    .expect("the run ends");
    let node = Node::start(Config {
        region: Region::Stored,
        name: "other".parse().unwrap(),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    let renamed = Error::Renamed {
        stored: "edge".parse().unwrap(),
        given: "other".parse().unwrap(),
    };
    assert_eq!(node.join(), Err(renamed));
    let listed = listed(&mut sim, &host, "");
    let named = [crate::name::FILE, "name.new"].map(PathBuf::from);
    assert!(
        !listed.iter().any(|path| named.contains(path)),
        "{listed:?}"
    );
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    assert_eq!(kept(&mut sim, &host), Some(edge));
}

/// A start with no region and another name, on a data directory that stores a
/// founding and no file `name`, stops and writes no name.
#[test]
fn a_start_with_no_region_and_another_name_after_the_name_file_is_gone_stops() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let node = Node::start(Config {
        name: "other".parse().unwrap(),
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run(), Ok(()));
    let renamed = Error::Renamed {
        stored: "edge".parse().unwrap(),
        given: "other".parse().unwrap(),
    };
    assert_eq!(node.join(), Err(renamed));
    assert_eq!(kept(&mut sim, &host), None);
}

/// With no file `name`, [`crate::name`] gives the name in the node's card in the
/// stored founding, and a start with it runs.
#[test]
fn the_name_after_the_name_file_is_gone_is_the_card_name() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let edge = remove_name(&mut sim, &host);
    let named = sim.run_on(&host, |host, _| async move {
        crate::name(&host.files(), None).await
    });
    let named = named.expect("the run ends").expect("names the node");
    assert_eq!(named, "edge".parse::<types::name::Name>().unwrap());
    let node = Node::start(Config {
        region: Region::Stored,
        name: named,
        ..config(&host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    assert_eq!(node.join(), Ok(()));
    assert_eq!(kept(&mut sim, &host), Some(edge));
}

/// A crash at each time of a start with no file `name` leaves a data directory that
/// a start with the card name runs and a start with another name refuses.
#[test]
fn a_crash_while_the_name_file_is_gone_leaves_a_directory_that_the_card_name_runs() {
    let other: types::name::Name = "other".parse().unwrap();
    for crash in [sim::Crash::Process, sim::Crash::Power] {
        let mut unkept = 0;
        for after in (0..).step_by(25_000).map(Span::from_nanos) {
            let mut sim = sim::Sim::new(sim::Config::default());
            let host = keyed(&mut sim, 2);
            assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
            remove_name(&mut sim, &host);
            let node = start(&host);
            assert_eq!(sim.run_for(after), Ok(()), "{crash:?} at {after:?}");
            sim.crash(&host, crash);
            drop(node);
            let written = kept(&mut sim, &host);
            unkept += usize::from(written.is_none());
            let node = Node::start(Config {
                region: Region::Stored,
                name: other.clone(),
                ..config(&host, Size::MEBIBYTE, Box::new(heap))
            });
            assert_eq!(sim.run(), Ok(()), "{crash:?} at {after:?}");
            let renamed = Error::Renamed {
                stored: "edge".parse().unwrap(),
                given: other.clone(),
            };
            assert_eq!(node.join(), Err(renamed), "{crash:?} at {after:?}");
            assert_eq!(
                start_and_stop(&mut sim, &host),
                Ok(()),
                "{crash:?} at {after:?}"
            );
            if written.is_some() {
                break;
            }
        }
        assert!(unkept > 0, "{crash:?}: no crash came before the write");
    }
}

/// A start with no file `name`, whose founding does not read, stops the node and
/// writes no name.
#[test]
fn a_founding_that_does_not_read_after_the_name_file_is_gone_stops_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let operation = env::files::Operation::ReadAt;
    host.fail_file(Path::new(FOUNDING), operation);
    let error = ::mesh::Error::Files(env::files::Error::Io {
        path: PathBuf::from(FOUNDING),
        operation,
        code: 5,
    });
    assert_eq!(start_and_stop(&mut sim, &host), Err(Error::Mesh(error)));
    assert_eq!(kept(&mut sim, &host), None);
}

/// With no file `name`, [`crate::name`] gives the error of a founding that does not
/// read.
#[test]
fn a_founding_that_does_not_read_after_the_name_file_is_gone_fails_the_name() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let operation = env::files::Operation::ReadAt;
    host.fail_file(Path::new(FOUNDING), operation);
    let error = ::mesh::Error::Files(env::files::Error::Io {
        path: PathBuf::from(FOUNDING),
        operation,
        code: 5,
    });
    assert_eq!(named(&mut sim, &host), Err(Error::Mesh(error)));
}

/// With no file `name` and no file `node.key`, [`crate::name`] reads the founding, so
/// a founding that does not read gives its error.
#[test]
fn a_founding_that_does_not_read_with_no_node_key_fails_the_name() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    remove(&mut sim, &host, identity::FILE);
    let operation = env::files::Operation::ReadAt;
    host.fail_file(Path::new(FOUNDING), operation);
    let error = ::mesh::Error::Files(env::files::Error::Io {
        path: PathBuf::from(FOUNDING),
        operation,
        code: 5,
    });
    assert_eq!(named(&mut sim, &host), Err(Error::Mesh(error)));
}

/// With no file `name`, a list of the mesh's directory that fails gives
/// [`Error::Directory`].
#[test]
fn a_mesh_directory_that_does_not_list_gives_its_error() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let operation = env::files::Operation::List;
    host.fail_file(Path::new("mesh"), operation);
    let error = env::files::Error::Io {
        path: PathBuf::from("mesh"),
        operation,
        code: 5,
    };
    assert_eq!(named(&mut sim, &host), Err(Error::Directory(error)));
}

/// With no file `name`, a founding that does not read gives its error before a file
/// `node.key` that a node did not write.
#[test]
fn a_founding_that_does_not_read_comes_before_a_node_key_that_is_not_a_key() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    remove(&mut sim, &host, identity::FILE);
    write(&mut sim, &host, identity::FILE, Vec::new());
    let operation = env::files::Operation::ReadAt;
    host.fail_file(Path::new(FOUNDING), operation);
    let error = ::mesh::Error::Files(env::files::Error::Io {
        path: PathBuf::from(FOUNDING),
        operation,
        code: 5,
    });
    assert_eq!(named(&mut sim, &host), Err(Error::Mesh(error)));
}

/// With no file `name`, no file `node.key` or the key of a node with no card in the
/// founding gives [`Error::NoCard`], and a file `node.key` that a node did not write
/// gives [`Error::Key`].
#[test]
fn the_card_name_is_the_one_of_the_key_in_node_key() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let own = bytes(&mut sim, &host, identity::FILE).expect("the file");
    let another = another_key(&own);
    let mut foreign = own.clone();
    foreign[40] ^= 1;
    let cases = [
        (None, Err(Error::NoCard)),
        (Some(another), Err(Error::NoCard)),
        (Some(foreign), Err(Error::Key)),
        (Some(Vec::new()), Err(Error::Key)),
        (Some(vec![0; identity::LEN]), Err(Error::Key)),
        (Some(own), Ok("edge".parse().unwrap())),
    ];
    for (key, name) in cases {
        remove(&mut sim, &host, identity::FILE);
        if let Some(key) = key.clone() {
            write(&mut sim, &host, identity::FILE, key);
        }
        assert_eq!(named(&mut sim, &host), name, "{key:?}");
    }
}

/// With no file `name` and no founding, a file `node.key` that a node did not write
/// gives [`Error::Key`], not [`Error::Unnamed`], and a start stops and writes no name.
#[test]
fn a_node_key_that_is_not_a_key_comes_before_unnamed() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = sim.node(sim::node::Config::default());
    write_key(&mut sim, &host, Vec::new());
    assert_eq!(named(&mut sim, &host), Err(Error::Key));
    let found = start_as(&mut sim, &host, Region::Absent, "edge");
    assert_eq!(found, Err(Error::Key));
    assert_eq!(kept(&mut sim, &host), None);
}

/// A start with no file `name`, whose file `node.key` does not read, stops the node
/// and writes no name.
#[test]
fn a_node_key_that_does_not_read_after_the_name_file_is_gone_stops_the_node() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let operation = env::files::Operation::ReadAt;
    host.fail_file(Path::new(identity::FILE), operation);
    let error = env::files::Error::Io {
        path: PathBuf::from(identity::FILE),
        operation,
        code: 5,
    };
    assert_eq!(
        start_and_stop(&mut sim, &host),
        Err(Error::Directory(error))
    );
    assert_eq!(kept(&mut sim, &host), None);
}

/// A file `name` gives the stored name, also when the node's card holds another one.
#[test]
fn the_name_file_comes_before_the_card_name() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let mut body = [0; crate::name::LEN - 4];
    body[..17].copy_from_slice(b"foundation/name/1");
    body[17] = 5;
    body[18..23].copy_from_slice(b"other");
    let file: [u8; crate::name::LEN] = crate::sector::summed(&body).expect("a body");
    write(&mut sim, &host, crate::name::FILE, file.to_vec());
    assert_eq!(named(&mut sim, &host), Ok("other".parse().unwrap()));
}

/// Starts the node on `host` with `region` and the name `name`, runs `sim` for a
/// second, then stops it and gives the error of its join.
#[expect(
    clippy::unwrap_in_result,
    reason = "a test helper panics on a name that does not parse"
)]
fn start_as(
    sim: &mut sim::Sim,
    host: &sim::node::Node,
    region: Region,
    name: &str,
) -> Result<(), Error> {
    let node = Node::start(Config {
        region,
        name: name.parse().unwrap(),
        ..config(host, Size::MEBIBYTE, Box::new(heap))
    });
    assert_eq!(sim.run_for(Span::SECOND), Ok(()));
    node.stop();
    assert_eq!(sim.run(), Ok(()));
    node.join()
}

/// A key of the node that has no card in the founding of [`root`].
fn another_key(own: &[u8]) -> Vec<u8> {
    let mut another: [u8; identity::LEN] = own.try_into().unwrap();
    another[31] ^= 1;
    crate::sector::checksum(&mut another);
    another.to_vec()
}

/// A start with no file `name` and no file `node.key`, on a data directory that stores
/// a founding, stops and writes nothing. With the key back, the card name runs.
#[test]
fn a_start_with_no_name_file_and_no_node_key_stops_and_writes_nothing() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let own = bytes(&mut sim, &host, identity::FILE).expect("the file");
    remove(&mut sim, &host, identity::FILE);
    let found = start_as(&mut sim, &host, Region::Stored, "other");
    assert_eq!(found, Err(Error::NoCard));
    assert_eq!(kept(&mut sim, &host), None);
    assert_eq!(bytes(&mut sim, &host, identity::FILE), None);
    write(&mut sim, &host, identity::FILE, own);
    let renamed = Error::Renamed {
        stored: "edge".parse().unwrap(),
        given: "other".parse().unwrap(),
    };
    let found = start_as(&mut sim, &host, Region::Stored, "other");
    assert_eq!(found, Err(renamed));
    assert_eq!(start_as(&mut sim, &host, Region::Stored, "edge"), Ok(()));
}

/// A start with no file `name` and the key of another node in `node.key`, on a data
/// directory that stores a founding, stops and writes nothing.
#[test]
fn a_start_with_no_name_file_and_a_key_with_no_card_stops_and_writes_nothing() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    let own = bytes(&mut sim, &host, identity::FILE).expect("the file");
    let another = another_key(&own);
    remove(&mut sim, &host, identity::FILE);
    write(&mut sim, &host, identity::FILE, another.clone());
    let found = start_as(&mut sim, &host, Region::Stored, "edge");
    assert_eq!(found, Err(Error::NoCard));
    assert_eq!(kept(&mut sim, &host), None);
    assert_eq!(bytes(&mut sim, &host, identity::FILE), Some(another));
}

/// A start with [`Region::Absent`] and no file `name` and no file `node.key`, on a
/// data directory that stores a founding, stops and writes nothing.
#[test]
fn a_start_with_no_region_and_no_node_key_on_a_stored_founding_stops() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    remove_name(&mut sim, &host);
    remove(&mut sim, &host, identity::FILE);
    let found = start_as(&mut sim, &host, Region::Absent, "edge");
    assert_eq!(found, Err(Error::NoCard));
    assert_eq!(kept(&mut sim, &host), None);
    assert_eq!(bytes(&mut sim, &host, identity::FILE), None);
}

/// A start with a file `name` and no file `node.key`, on a data directory that stores
/// a founding, stops with each region and makes no new key: a new key is a new node
/// to its region.
#[test]
fn a_start_with_a_name_file_and_no_node_key_on_a_stored_founding_stops() {
    for region in [Region::Stored, Region::Absent] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
        remove(&mut sim, &host, identity::FILE);
        let found = start_as(&mut sim, &host, region.clone(), "edge");
        assert_eq!(found, Err(Error::NoCard), "{region:?}");
        assert_eq!(bytes(&mut sim, &host, identity::FILE), None, "{region:?}");
    }
}

/// A start with a name on a data directory with only an empty directory `mesh` stops
/// with each region before it writes a file.
#[test]
fn a_start_on_a_mesh_directory_with_no_node_key_writes_nothing() {
    for region in [Region::Stored, Region::Absent] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = host(&mut sim, 2);
        sim.run_on(&host, |host, _| async move {
            let files = host.files();
            files
                .create_dir(&crate::directory::mesh())
                .await
                .expect("makes");
            files.sync_dir(Path::new("")).await.expect("syncs");
        })
        .expect("the run ends");
        let found = start_as(&mut sim, &host, region.clone(), "edge");
        assert_eq!(found, Err(Error::NoCard), "{region:?}");
        assert_eq!(kept(&mut sim, &host), None, "{region:?}");
        assert_eq!(bytes(&mut sim, &host, identity::FILE), None, "{region:?}");
    }
}

/// A start with a file `name` and no file `node.key` or `budget`, on a data directory
/// that stores a founding, stops with each region before it writes `budget`.
#[test]
fn a_start_with_no_node_key_and_no_budget_on_a_stored_founding_writes_nothing() {
    for region in [Region::Stored, Region::Absent] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
        remove(&mut sim, &host, identity::FILE);
        remove(&mut sim, &host, crate::budget::FILE);
        let found = start_as(&mut sim, &host, region.clone(), "edge");
        assert_eq!(found, Err(Error::NoCard), "{region:?}");
        assert_eq!(
            bytes(&mut sim, &host, crate::budget::FILE),
            None,
            "{region:?}"
        );
        assert_eq!(bytes(&mut sim, &host, identity::FILE), None, "{region:?}");
    }
}

/// A power cut of the first start after the mesh's directory and before its founding,
/// then a loss of `name` and `node.key`: the start gives [`Error::NoCard`] and writes
/// no file `name`, so the node starts with its name once `node.key` is back.
#[test]
fn a_no_card_start_after_a_cut_before_the_founding_writes_no_name() {
    for after in (0..).step_by(5_000).map(Span::from_nanos) {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        let node = start(&host);
        assert_eq!(sim.run_for(after), Ok(()));
        sim.crash(&host, sim::Crash::Power);
        drop(node);
        let mesh = listed(&mut sim, &host, "").contains(&PathBuf::from("mesh"));
        if !mesh || stored(&mut sim, &host).is_some() {
            assert!(
                after < Span::SECOND,
                "no cut between the directory and founding"
            );
            continue;
        }
        remove(&mut sim, &host, crate::name::FILE);
        let own = bytes(&mut sim, &host, identity::FILE).expect("the file");
        remove(&mut sim, &host, identity::FILE);
        let found = start_as(&mut sim, &host, Region::Stored, "other");
        assert_eq!(found, Err(Error::NoCard), "{after:?}");
        assert_eq!(kept(&mut sim, &host), None, "{after:?}");
        assert_eq!(bytes(&mut sim, &host, identity::FILE), None, "{after:?}");
        write(&mut sim, &host, identity::FILE, own);
        let found = start_as(&mut sim, &host, Region::Stored, "edge");
        assert_eq!(found, Ok(()), "{after:?}");
        return;
    }
}

/// A start with a file `name` and no file `node.key`, whose list of the mesh's
/// directory fails, gives [`Error::Directory`] and makes no new key.
#[test]
fn a_mesh_directory_that_does_not_list_with_no_node_key_stops_the_start() {
    for region in [Region::Stored, Region::Absent] {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = keyed(&mut sim, 2);
        assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
        remove(&mut sim, &host, identity::FILE);
        let operation = env::files::Operation::List;
        host.fail_file(Path::new("mesh"), operation);
        let error = env::files::Error::Io {
            path: PathBuf::from("mesh"),
            operation,
            code: 5,
        };
        let found = start_as(&mut sim, &host, region.clone(), "edge");
        assert_eq!(found, Err(Error::Directory(error)), "{region:?}");
        assert_eq!(bytes(&mut sim, &host, identity::FILE), None, "{region:?}");
    }
}

/// A start with a file `name` and the key of another node in `node.key`, on a data
/// directory that stores a founding: the open of the region refuses the key, and a
/// start with [`Region::Absent`] opens no region.
#[test]
fn a_key_with_no_card_and_a_name_file_fails_only_the_open_of_the_region() {
    let mut sim = sim::Sim::new(sim::Config::default());
    let host = keyed(&mut sim, 2);
    assert_eq!(start_and_stop(&mut sim, &host), Ok(()));
    let own = bytes(&mut sim, &host, identity::FILE).expect("the file");
    remove(&mut sim, &host, identity::FILE);
    write(&mut sim, &host, identity::FILE, another_key(&own));
    let key = sim.run_on(&host, |host, _| async move {
        identity::key(&host.files()).await.unwrap().expect("a key")
    });
    let error = mesh::Error::NotMember(key.expect("the run ends"));
    let found = start_as(&mut sim, &host, Region::Stored, "edge");
    assert_eq!(found, Err(Error::Mesh(error)));
    assert_eq!(start_as(&mut sim, &host, Region::Absent, "edge"), Ok(()));
}
