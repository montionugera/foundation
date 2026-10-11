//! A file of the data directory that fits one sector. It starts with a tag, and its
//! last 4 bytes are the CRC32C of the bytes before them (little-endian). A file that
//! [`publish`] makes is whole or not there, also to a read while the write runs.

use std::path::{Path, PathBuf};

use env::files::{File, Files, Mode};

/// The pool of the file's block. A call can come while the shard's buffer holds the
/// blocks of the shard's pool, so that pool can lack room for it.
const POOL: block::Config = match block::Config::new(4096) {
    Ok(config) => config,
    Err(_) => panic!("invariant: a sector fits"),
};

/// What a file of `N` bytes holds.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Held<const N: usize> {
    /// No file.
    Nothing,
    /// The bytes that a node wrote, with the tag and a checksum that matches.
    Written([u8; N]),
    /// A file that no node wrote: another length, tag, or checksum.
    Foreign,
}

/// What `path` in `files`, which [`publish`] makes, holds. Opens it to read only, so
/// it makes nothing and waits for no lock.
///
/// # Errors
///
/// The error of each file call that fails, except [`env::files::Error::NotFound`],
/// which gives [`Held::Nothing`].
pub(crate) async fn read<const N: usize>(
    files: &Files,
    path: &Path,
    tag: &[u8],
) -> Result<Held<N>, env::files::Error> {
    let file = match files.open(path, Mode::Read).await {
        Ok(file) => file,
        Err(env::files::Error::NotFound { .. }) => return Ok(Held::Nothing),
        Err(error) => return Err(error),
    };
    if file.len() != N as u64 {
        return Ok(Held::Foreign);
    }
    let bytes = bytes(&file).await?;
    Ok(written(&bytes, tag).map_or(Held::Foreign, Held::Written))
}

/// Makes `path` in `files` with `bytes`, which [`checksum`] completed, and makes it
/// durable: removes the file `<path>.new` that a crash left, writes `bytes` to a new
/// one, and renames it to `path`. Call it under the lock of the data directory, when
/// `path` is not there.
///
/// # Errors
///
/// The error of the first file call that fails. [`env::files::Error::Exists`] when
/// `path` is there.
pub(crate) async fn publish<const N: usize>(
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

/// `bytes`, when a node wrote them to a file whose tag is `tag`: they start with the
/// tag, and their checksum matches.
pub(crate) fn written<const N: usize>(bytes: &[u8; N], tag: &[u8]) -> Option<[u8; N]> {
    let (body, sum) = bytes.split_at(N - 4);
    (body.starts_with(tag) && crc32c::crc32c(body).to_le_bytes() == *sum)
        .then_some(*bytes)
}

/// Writes the CRC32C of the bytes of `bytes` before its last 4 into those 4.
pub(crate) fn checksum<const N: usize>(bytes: &mut [u8; N]) {
    let (body, sum) = bytes.split_at_mut(N - 4);
    sum.copy_from_slice(&crc32c::crc32c(body).to_le_bytes());
}

/// Checks that `bytes` start with `tag` and match their checksum exactly when
/// `encode` gives them back: `encode` makes the file again from the fields of
/// `bytes`, whatever their tag and checksum.
///
/// # Panics
///
/// When the check fails.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn check<const N: usize>(
    bytes: &[u8; N],
    tag: &[u8],
    encode: impl Fn(&[u8; N]) -> [u8; N],
) {
    let valid = encode(bytes) == *bytes;
    match written(bytes, tag) {
        Some(_) => assert!(valid, "decodes {bytes:02x?}"),
        None => assert!(!valid, "refuses {bytes:02x?}"),
    }
}

/// `body` with its CRC32C appended, or `None` when `body` is not `N - 4` bytes.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn summed<const N: usize>(body: &[u8]) -> Option<[u8; N]> {
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
    let pool = block::Pool::heap(POOL);
    let into = pool.alloc(N).expect("invariant: the pool holds a sector");
    let read = file.read_at(0, into).await?;
    Ok((&*read).try_into().expect("invariant: a read fills it"))
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
        check(&file(), b"t", encode);
        let mut other = file();
        other[1] ^= 1;
        check(&other, b"t", encode);
    }

    #[test]
    #[should_panic(expected = "decodes [74, 01, 02, 03,")]
    fn panics_when_a_decode_gives_bytes_that_encode_does_not() {
        check(&file(), b"t", |_| [0; 12]);
    }

    #[test]
    #[should_panic(expected = "refuses [74, 01, 02, 03,")]
    fn panics_when_a_decode_refuses_bytes_that_encode_gives() {
        check(&file(), b"u", encode);
    }

    #[test]
    fn refuses_a_body_of_another_length() {
        assert_eq!(summed::<8>(&[1, 2, 3]), None);
        assert_eq!(summed::<8>(&[0; 5]), None);
    }
}
