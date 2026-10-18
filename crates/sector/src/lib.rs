//! The files of a data directory that fit one sector. Each starts with a tag, and its
//! last 4 bytes are the CRC32C of the bytes before them (little-endian). A file that
//! [`publish`] makes is whole or not there, also to a read while the write runs. It
//! holds the formats of [`address`] and [`admin`], which a program on the node's host
//! reads with [`local`].
//!
//! The last byte of a tag is the version of its form, one ASCII digit or letter, and
//! the bytes before it are the tag's prefix. A new form takes another version, so no
//! tag is the prefix of another.

pub mod address;
pub mod admin;

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

use env::files::{File, Files, Mode};
use types::ed25519::{PrivateKey, PublicKey};

/// The pool of the file's block. A call can come while the shard's buffer holds the
/// blocks of the shard's pool, so that pool can lack room for it.
const POOL: block::Config = match block::Config::new(4096) {
    Ok(config) => config,
    Err(_) => panic!("invariant: a sector fits"),
};

/// What a file of `N` bytes holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Held<const N: usize> {
    /// No file.
    Nothing,
    /// The bytes that a node wrote, with the tag and a checksum that matches.
    Written([u8; N]),
    /// A file of any length that holds the prefix of the tag and the version in the
    /// byte after it, which is not the version of the tag: a node of another version
    /// wrote it.
    Version(u8),
    /// A file that no node wrote: shorter than the tag, another prefix, or the tag
    /// with another length or checksum.
    Foreign,
}

/// The node that runs on a data directory, and the key of its first admin.
#[derive(Clone, Debug)]
pub struct Local {
    /// The node's key.
    pub key: types::node::Key,
    /// The public key that the node's transport proves.
    pub public_key: PublicKey,
    /// Where a program on the node's host dials the node: the address where its port
    /// is bound, with the loopback IP of its family in place of an unspecified IP.
    pub address: SocketAddr,
    /// The private key of the first admin of the root region.
    pub admin: PrivateKey,
}

/// Why [`local`] gave nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// No node runs on the directory: its file `address` holds no address of a node.
    NotRunning,
    /// The directory holds no file `admin.key` that a node wrote.
    Key,
    /// The file `file` holds the format of version `found`, and this program reads
    /// version `expected`: a node of another version wrote it.
    Version {
        /// The name of the file.
        file: &'static str,
        /// The version byte that the file holds, which can be any byte.
        found: u8,
        /// The version byte of the format that this program reads.
        expected: u8,
    },
    /// A file call on the directory failed.
    Files(env::files::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRunning => f.write_str("no node runs on the data directory"),
            Self::Key => {
                f.write_str("the data directory holds no admin.key that a node wrote")
            }
            Self::Version {
                file,
                found,
                expected,
            } => write!(
                f,
                "{file} holds format version {}, and this program reads version {}",
                found.escape_ascii(),
                expected.escape_ascii(),
            ),
            Self::Files(error) => write!(f, "{error}"),
        }
    }
}

/// The node that runs on the data directory of `files`, and its first admin. Opens
/// each file to read only, so it makes nothing and waits for no lock.
///
/// # Errors
///
/// [`Error::Version`] when a file holds the form of another version, else
/// [`Error::NotRunning`] when `address` holds no address of a node, [`Error::Key`]
/// when no node wrote `admin.key`, and [`Error::Files`] for a file call that fails,
/// other than [`env::files::Error::NotFound`].
pub async fn local(files: &Files) -> Result<Local, Error> {
    let path = Path::new(address::FILE);
    let held = read(files, path, address::TAG)
        .await
        .map_err(Error::Files)?;
    let address = match held {
        Held::Written(bytes) => address::decode(&bytes),
        Held::Version(found) => {
            return Err(version(address::FILE, address::TAG, found));
        }
        Held::Nothing | Held::Foreign => None,
    };
    let address = address.ok_or(Error::NotRunning)?;
    let path = Path::new(admin::FILE);
    let held = read(files, path, admin::TAG).await.map_err(Error::Files)?;
    let admin = match held {
        Held::Written(bytes) => admin::decode(&bytes),
        Held::Version(found) => return Err(version(admin::FILE, admin::TAG, found)),
        Held::Nothing | Held::Foreign => None,
    };
    Ok(Local {
        key: address.key,
        public_key: address.public_key,
        address: dialed(address.bound),
        admin: admin.ok_or(Error::Key)?,
    })
}

/// The [`Error::Version`] of the file `file`, whose tag is `tag`, that holds the
/// version `found`.
fn version(file: &'static str, tag: &[u8], found: u8) -> Error {
    Error::Version {
        file,
        found,
        expected: tag[tag.len() - 1],
    }
}

/// Where a program on the node's host dials `bound`: the loopback IP of its family in
/// place of the unspecified IP.
fn dialed(mut bound: SocketAddr) -> SocketAddr {
    match bound.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            bound.set_ip(Ipv4Addr::LOCALHOST.into());
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            bound.set_ip(Ipv6Addr::LOCALHOST.into());
        }
        IpAddr::V4(_) | IpAddr::V6(_) => {}
    }
    bound
}

/// What `path` in `files`, which [`publish`] makes, holds. Opens it to read only, so
/// it makes nothing and waits for no lock.
///
/// # Errors
///
/// The error of each file call that fails, except [`env::files::Error::NotFound`],
/// which gives [`Held::Nothing`].
pub async fn read<const N: usize>(
    files: &Files,
    path: &Path,
    tag: &[u8],
) -> Result<Held<N>, env::files::Error> {
    let file = match files.open(path, Mode::Read).await {
        Ok(file) => file,
        Err(env::files::Error::NotFound { .. }) => return Ok(Held::Nothing),
        Err(error) => return Err(error),
    };
    if file.len() == N as u64 {
        let bytes = bytes(&file).await?;
        return Ok(if written(&bytes, tag) {
            Held::Written(bytes)
        } else {
            other(&bytes, tag)
        });
    }
    if file.len() < tag.len() as u64 {
        return Ok(Held::Foreign);
    }
    Ok(other(&head(&file, tag.len()).await?, tag))
}

/// [`Held::Version`] when `bytes` hold the prefix of `tag` and another version byte
/// after it, else [`Held::Foreign`].
fn other<const N: usize>(bytes: &[u8], tag: &[u8]) -> Held<N> {
    let (prefix, version) = tag.split_at(tag.len() - 1);
    match bytes
        .get(..tag.len())
        .map(|head| head.split_at(prefix.len()))
    {
        Some((held, [found])) if held == prefix && found != &version[0] => {
            Held::Version(*found)
        }
        _ => Held::Foreign,
    }
}

/// Makes `path` in `files` with `bytes`, which [`checksum`] completed, and makes it
/// durable: removes the file `<path>.new` that a crash left, writes `bytes` to a new
/// one, and renames it to `path`. Call it when `path` is not there and no other call
/// writes `path` or `<path>.new`: under the lock of the data directory, or on one that
/// no node runs on. A failed call can leave `<path>.new`, which the next call removes.
///
/// # Errors
///
/// The error of the first file call that fails.
pub async fn publish<const N: usize>(
    files: &Files,
    path: &Path,
    bytes: &[u8; N],
) -> Result<(), env::files::Error> {
    let mut staged = path.as_os_str().to_owned();
    staged.push(".new");
    let staged = PathBuf::from(staged);
    files.remove(&staged).await?;
    let mut file = files.open(&staged, Mode::Create { len: N as u64 }).await?;
    file.write_at(0, &[block(bytes)]).await?;
    file.rename(path).await?;
    files.sync_dir(Path::new("")).await
}

/// Writes `bytes`, which [`checksum`] completed, to `file` of `files`, a file of `N`
/// bytes open to write, and makes the file and its name durable.
///
/// # Errors
///
/// The error of the first file call that fails.
pub async fn write<const N: usize>(
    files: &Files,
    file: &File,
    bytes: &[u8; N],
) -> Result<(), env::files::Error> {
    file.write_at(0, &[block(bytes)]).await?;
    file.sync().await?;
    files.sync_dir(Path::new("")).await
}

/// Writes `N` zero bytes to `file`, a file of `N` bytes open to write, with no sync.
///
/// # Errors
///
/// The error of the write.
pub async fn clear<const N: usize>(file: &File) -> Result<(), env::files::Error> {
    file.write_at(0, &[block(&[0; N])]).await
}

/// Whether a node wrote `bytes` to a file whose tag is `tag`: they start with the tag,
/// and their checksum matches.
#[must_use]
pub fn written<const N: usize>(bytes: &[u8; N], tag: &[u8]) -> bool {
    let (body, sum) = bytes.split_at(N - 4);
    body.starts_with(tag) && crc32c::crc32c(body).to_le_bytes() == *sum
}

/// Writes the CRC32C of the bytes of `bytes` before its last 4 into those 4.
pub fn checksum<const N: usize>(bytes: &mut [u8; N]) {
    let (body, sum) = bytes.split_at_mut(N - 4);
    sum.copy_from_slice(&crc32c::crc32c(body).to_le_bytes());
}

/// Checks that `bytes` start with `tag`, match their checksum, and hold fields that a
/// value has exactly when `encode` gives them back: `encode` makes the file again from
/// the fields of `bytes`, whatever their tag and checksum, or gives `None` for fields
/// that no value has.
///
/// # Panics
///
/// When the check fails.
#[cfg(any(test, feature = "sim"))]
pub fn check<const N: usize>(
    bytes: &[u8; N],
    tag: &[u8],
    encode: impl Fn(&[u8; N]) -> Option<[u8; N]>,
) {
    let encoded = encode(bytes);
    let valid = encoded == Some(*bytes);
    if written(bytes, tag) && encoded.is_some() {
        assert!(valid, "decodes {bytes:02x?}");
    } else {
        assert!(!valid, "refuses {bytes:02x?}");
    }
}

/// `body` with its CRC32C appended, or `None` when `body` is not `N - 4` bytes.
#[cfg(any(test, feature = "sim"))]
#[must_use]
pub fn summed<const N: usize>(body: &[u8]) -> Option<[u8; N]> {
    if body.len() != N - 4 {
        return None;
    }
    let mut bytes = [0; N];
    bytes[..N - 4].copy_from_slice(body);
    checksum(&mut bytes);
    Some(bytes)
}

/// The `N` bytes of `file`, which has `N` bytes.
async fn bytes<const N: usize>(file: &File) -> Result<[u8; N], env::files::Error> {
    const { assert!(N <= env::files::SECTOR, "the file fits one sector") };
    let read = head(file, N).await?;
    Ok((&*read).try_into().expect("invariant: a read fills it"))
}

/// The first `len` bytes of `file`, which has at least `len` bytes, up to a sector.
async fn head(file: &File, len: usize) -> Result<block::Unique, env::files::Error> {
    let pool = block::Pool::heap(POOL);
    let into = pool.alloc(len).expect("invariant: the pool holds a sector");
    file.read_at(0, into).await
}

/// A block of `bytes`.
fn block<const N: usize>(bytes: &[u8; N]) -> block::Block {
    const { assert!(N <= env::files::SECTOR, "the file fits one sector") };
    let pool = block::Pool::heap(POOL);
    pool.copy(bytes)
        .expect("invariant: the pool holds a sector")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    proptest! {
        #[test]
        fn dialed_keeps_each_specified_ip(
            ip in any::<IpAddr>().prop_filter("specified", |ip| !ip.is_unspecified()),
            port: u16,
        ) {
            let bound = SocketAddr::new(ip, port);
            prop_assert_eq!(dialed(bound), bound);
        }

        #[test]
        fn dialed_gives_the_loopback_ip_of_the_family_for_an_unspecified_ip(port: u16) {
            let v4 = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
            let v6 = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port);
            let loopback = |ip: IpAddr| SocketAddr::new(ip, port);
            prop_assert_eq!(dialed(v4), loopback(Ipv4Addr::LOCALHOST.into()));
            prop_assert_eq!(dialed(v6), loopback(Ipv6Addr::LOCALHOST.into()));
        }
    }

    #[test]
    fn sums_a_body_of_n_minus_4_bytes() {
        let bytes: [u8; 8] = summed(&[1, 2, 3, 4]).expect("a body");
        assert_eq!(bytes[..4], [1, 2, 3, 4]);
        assert_eq!(bytes[4..], crc32c::crc32c(&[1, 2, 3, 4]).to_le_bytes());
    }

    /// The bytes of a file of 12 bytes with the tag `t` and the field in `bytes[1..4]`.
    fn encode(bytes: &[u8; 12]) -> [u8; 12] {
        let mut file = [0; 12];
        file[0] = b't';
        file[1..4].copy_from_slice(&bytes[1..4]);
        checksum(&mut file);
        file
    }

    /// A file that [`encode`] gives, with the field `[1, 2, 3]`.
    fn file() -> [u8; 12] {
        encode(&[0, 1, 2, 3, 0, 0, 0, 0, 0, 0, 0, 0])
    }

    #[test]
    fn checks_a_file_that_encode_gives_back() {
        check(&file(), b"t", |bytes| Some(encode(bytes)));
        let mut other = file();
        other[1] ^= 1;
        check(&other, b"t", |bytes| Some(encode(bytes)));
        check(&file(), b"u", |_| None);
        check(&file(), b"t", |_| None);
    }

    #[test]
    #[should_panic(expected = "decodes [74, 01, 02, 03,")]
    fn panics_when_a_decode_gives_bytes_that_encode_does_not() {
        check(&file(), b"t", |_| Some([0; 12]));
    }

    #[test]
    #[should_panic(expected = "refuses [74, 01, 02, 03,")]
    fn panics_when_a_decode_refuses_bytes_that_encode_gives() {
        check(&file(), b"u", |bytes| Some(encode(bytes)));
    }

    #[test]
    fn refuses_a_body_of_another_length() {
        assert_eq!(summed::<8>(&[1, 2, 3]), None);
        assert_eq!(summed::<8>(&[0; 5]), None);
    }

    /// `body` with its CRC32C appended.
    fn sealed(body: &[u8]) -> Vec<u8> {
        let mut bytes = body.to_vec();
        bytes.extend_from_slice(&crc32c::crc32c(body).to_le_bytes());
        bytes
    }

    #[test]
    fn the_tag_of_each_format_ends_in_version_1() {
        assert_eq!(address::TAG.last(), Some(&b'1'));
        assert_eq!(admin::TAG.last(), Some(&b'1'));
    }

    #[test]
    fn version_prints_the_found_byte_as_an_escaped_ascii_char() {
        let version = |found| Error::Version {
            file: "address",
            found,
            expected: b'1',
        };
        assert_eq!(
            version(b'2').to_string(),
            "address holds format version 2, and this program reads version 1"
        );
        assert_eq!(
            version(0xff).to_string(),
            "address holds format version \\xff, and this program reads version 1"
        );
    }

    #[test]
    fn the_texts_of_the_other_errors() {
        assert_eq!(
            Error::NotRunning.to_string(),
            "no node runs on the data directory"
        );
        assert_eq!(
            Error::Key.to_string(),
            "the data directory holds no admin.key that a node wrote"
        );
        let error = env::files::Error::NotFound {
            path: PathBuf::from("address"),
        };
        assert_eq!(Error::Files(error.clone()).to_string(), error.to_string());
    }

    /// The bytes of `address` of a node at `bound`.
    fn address_file(bound: SocketAddr) -> [u8; address::LEN] {
        address::encode(&address::Address {
            key: types::node::Key::from_u128(7),
            public_key: PrivateKey([7; 32]).public(),
            bound,
        })
    }

    /// What [`local`] gives on a host whose data directory holds `files`, each a name
    /// and its bytes, once the run ends.
    fn local_over(
        files: Vec<(&'static str, Vec<u8>)>,
    ) -> Result<Result<Local, Error>, sim::Error> {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = sim.node(sim::node::Config::default());
        sim.run_on(&host, |host, _| async move {
            let directory = host.files();
            let pool = block::Pool::heap(POOL);
            for (name, bytes) in files {
                let len = bytes.len() as u64;
                let path = Path::new(name);
                let file = directory.open(path, Mode::Create { len }).await;
                let file = file.expect("makes the file");
                let block = pool.copy(&bytes).expect("a block");
                file.write_at(0, &[block]).await.expect("writes");
            }
            local(&directory).await
        })
    }

    #[test]
    fn local_gives_the_node_and_the_admin_that_a_node_wrote() {
        let bound = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 7400));
        let local = local_over(vec![
            (address::FILE, address_file(bound).to_vec()),
            (admin::FILE, admin::encode(&PrivateKey([3; 32])).to_vec()),
        ])
        .expect("the run ends")
        .expect("a node runs");
        assert_eq!(local.key, types::node::Key::from_u128(7));
        assert_eq!(local.public_key, PrivateKey([7; 32]).public());
        assert_eq!(local.address, SocketAddr::from((Ipv4Addr::LOCALHOST, 7400)));
        assert_eq!(local.admin.0, [3; 32]);
    }

    /// The [`Error::Version`] of `file` that holds version `2`.
    fn version_2(file: &'static str) -> Error {
        Error::Version {
            file,
            found: b'2',
            expected: b'1',
        }
    }

    /// What [`local`] gives, with each `Ok` dropped, on an `address` of `bytes`.
    fn local_over_address(bytes: Vec<u8>) -> Result<Result<(), Error>, sim::Error> {
        local_over(vec![(address::FILE, bytes)]).map(|read| read.map(drop))
    }

    /// `bytes` with `len` bytes, `9` after them.
    fn padded(bytes: &[u8], len: usize) -> Vec<u8> {
        let mut padded = bytes.to_vec();
        padded.resize(len, 9);
        padded
    }

    #[test]
    fn local_on_an_address_of_another_version_and_length_gives_version() {
        let bytes = sealed(&padded(b"foundation/address/2", 140));
        assert_eq!(local_over_address(bytes), Ok(Err(version_2("address"))));
    }

    #[test]
    fn local_on_an_address_of_another_version_and_its_length_gives_version() {
        let bytes = padded(b"foundation/address/2", address::LEN);
        assert_eq!(local_over_address(bytes), Ok(Err(version_2("address"))));
    }

    #[test]
    fn local_on_an_address_of_only_the_tag_of_another_version_gives_version() {
        let bytes = b"foundation/address/2".to_vec();
        assert_eq!(local_over_address(bytes), Ok(Err(version_2("address"))));
    }

    /// A file past a sector is read only up to the length of the tag.
    #[test]
    fn local_on_an_address_of_another_version_past_a_sector_gives_version() {
        let bytes = padded(b"foundation/address/2", env::files::SECTOR + 100);
        assert_eq!(local_over_address(bytes), Ok(Err(version_2("address"))));
    }

    #[test]
    fn local_on_an_admin_key_of_another_version_gives_version() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 7400));
        let error = local_over(vec![
            (address::FILE, address_file(bound).to_vec()),
            (admin::FILE, padded(b"foundation/admin/2", 60)),
        ]);
        let error = error.map(|read| read.map(drop));
        assert_eq!(error, Ok(Err(version_2("admin.key"))));
    }

    #[test]
    fn local_on_an_address_with_another_prefix_gives_not_running() {
        let bytes = sealed(&padded(b"foundation/other/2", 140));
        assert_eq!(local_over_address(bytes), Ok(Err(Error::NotRunning)));
        let bytes = padded(b"foundation/addresx2", address::LEN);
        assert_eq!(local_over_address(bytes), Ok(Err(Error::NotRunning)));
    }

    #[test]
    fn local_on_an_address_shorter_than_its_tag_gives_not_running() {
        let bytes = b"foundation/address/".to_vec();
        assert_eq!(local_over_address(bytes), Ok(Err(Error::NotRunning)));
    }

    #[test]
    fn local_on_an_address_of_version_1_and_another_length_gives_not_running() {
        let bytes = sealed(&padded(b"foundation/address/1", 140));
        assert_eq!(local_over_address(bytes), Ok(Err(Error::NotRunning)));
    }

    #[test]
    fn local_on_an_address_of_version_1_with_a_bad_checksum_gives_not_running() {
        let bound = SocketAddr::from((Ipv4Addr::LOCALHOST, 7400));
        let mut bytes = address_file(bound).to_vec();
        bytes[30] ^= 1;
        assert_eq!(local_over_address(bytes), Ok(Err(Error::NotRunning)));
    }

    /// What `path` holds after `change` runs on the files of a new host, as
    /// [`read`] gives it with the tag of `admin.key`.
    fn held_after<F>(
        change: impl FnOnce(Files) -> F + Send + 'static,
    ) -> Result<Result<Held<{ admin::LEN }>, env::files::Error>, sim::Error>
    where
        F: Future<Output = Result<(), env::files::Error>> + 'static,
    {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = sim.node(sim::node::Config::default());
        sim.run_on(&host, |host, _| async move {
            let files = host.files();
            change(files.clone()).await?;
            read(&files, Path::new(admin::FILE), admin::TAG).await
        })
    }

    #[test]
    fn publish_makes_the_file_that_read_gives() {
        let bytes = admin::encode(&PrivateKey([3; 32]));
        let held = held_after(move |files| async move {
            publish(&files, Path::new(admin::FILE), &bytes).await
        });
        assert_eq!(held, Ok(Ok(Held::Written(bytes))));
    }

    #[test]
    fn write_and_clear_change_the_bytes_that_read_gives() {
        let bytes = admin::encode(&PrivateKey([3; 32]));
        let len = admin::LEN as u64;
        let held = held_after(move |files| async move {
            let path = Path::new(admin::FILE);
            let file = files.open(path, Mode::Create { len }).await?;
            write(&files, &file, &bytes).await
        });
        assert_eq!(held, Ok(Ok(Held::Written(bytes))));
        let held = held_after(move |files| async move {
            let path = Path::new(admin::FILE);
            let file = files.open(path, Mode::Create { len }).await?;
            write(&files, &file, &bytes).await?;
            clear::<{ admin::LEN }>(&file).await
        });
        assert_eq!(held, Ok(Ok(Held::Foreign)));
    }
}
