//! The files of a data directory that fit one sector. Each starts with a tag, and its
//! last 4 bytes are the CRC32C of the bytes before them (little-endian). A file that
//! [`publish`] makes is whole or not there, also to a read while the write runs. It
//! holds the formats of [`address`] and [`admin`], which a program on the node's host
//! reads with [`local::read`].
//!
//! The last byte of a tag is the version of its format, and the bytes before it are
//! the tag's prefix. A version is one byte of lowercase ASCII, `1` to `9`, then `a` to
//! `z`, so no tag is the prefix of another
//! (<https://github.com/synnaxlabs/foundation/pull/3231#issuecomment-6252975559>).

pub mod address;
pub mod admin;
pub mod local;

use std::path::{Path, PathBuf};

use env::files::{File, Files, Mode};

/// The pool of the file's block. A call can come while the shard's buffer holds the
/// blocks of the shard's pool, so that pool can lack room for it.
const POOL: block::Config = match block::Config::new(4096) {
    Ok(config) => config,
    Err(_) => panic!("invariant: a sector fits"),
};

/// What a file holds, with `T` the value that a node wrote to it.
#[derive(Debug, PartialEq, Eq)]
pub enum Held<T> {
    /// No file.
    Nothing,
    /// The value that a node wrote, with the tag and a checksum that matches.
    Written(T),
    /// A file of any length that holds the prefix of the tag and the version in the
    /// byte after it, which is not the version of the tag: a node of another version
    /// wrote it.
    Version(u8),
    /// A file that no node wrote: shorter than the tag, another prefix, or the tag
    /// with another length or checksum.
    Foreign,
}

impl<T> Held<T> {
    /// The value of `f` on a written value, or [`Held::Foreign`] when `f` gives `None`.
    pub(crate) fn and_then<U>(self, f: impl FnOnce(T) -> Option<U>) -> Held<U> {
        match self {
            Self::Nothing => Held::Nothing,
            Self::Written(value) => f(value).map_or(Held::Foreign, Held::Written),
            Self::Version(found) => Held::Version(found),
            Self::Foreign => Held::Foreign,
        }
    }
}

/// What `path` in `files`, which [`publish`] or [`write`] makes, holds. Opens it to
/// read only, so it makes nothing and waits for no lock.
///
/// # Errors
///
/// The error of each file call that fails, except [`env::files::Error::NotFound`],
/// which gives [`Held::Nothing`].
///
/// # Panics
///
/// When `tag` is empty.
pub async fn read<const N: usize>(
    files: &Files,
    path: &Path,
    tag: &[u8],
) -> Result<Held<[u8; N]>, env::files::Error> {
    assert!(!tag.is_empty(), "a tag holds its version");
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
fn other<T>(bytes: &[u8], tag: &[u8]) -> Held<T> {
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
    use types::ed25519::PrivateKey;

    use super::*;

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

    #[test]
    fn the_tag_of_each_format_ends_in_version_1() {
        assert_eq!(address::TAG.last(), Some(&b'1'));
        assert_eq!(admin::TAG.last(), Some(&b'1'));
    }

    /// What `path` holds after `change` runs on the files of a new host, as
    /// [`read`] gives it with the tag of `admin.key`.
    fn held_after<F>(
        change: impl FnOnce(Files) -> F + Send + 'static,
    ) -> Result<Result<Held<[u8; admin::LEN]>, env::files::Error>, sim::Error>
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
    fn read_panics_on_an_empty_tag() {
        let mut sim = sim::Sim::new(sim::Config::default());
        let host = sim.node(sim::node::Config::default());
        let run = sim.run_on(&host, |host, _| async move {
            read::<{ admin::LEN }>(&host.files(), Path::new(admin::FILE), b"").await
        });
        let Err(sim::Error::Panicked { message, .. }) = run else {
            panic!("a panic: {run:?}");
        };
        assert_eq!(message, "a tag holds its version");
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
