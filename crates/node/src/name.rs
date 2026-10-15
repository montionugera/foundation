//! The node's name, in the file `name` of the data directory. The stored name is the
//! one in the file, else the one in the node's card in the founding that the data
//! directory stores.
//!
//! The file is 277 bytes: the tag, the length of the name in one byte, the name with
//! zero bytes after it up to [`Name::MAX_BYTES`], and the CRC32C of those 273 bytes
//! (little-endian), in one sector ([`crate::sector`]).

use std::path::Path;
use std::rc::Rc;

use env::files::Files;
use mesh::region::Founding;
use types::name::Name;

use crate::sector::{self, Held};
use crate::{Error, directory, identity, status};

/// The name of the file.
pub(crate) const FILE: &str = "name";
/// The most bytes of a node's name, so `<name>.<status>` fits a name for each name of
/// the status table.
pub(crate) const MAX_BYTES: usize = Name::MAX_BYTES - 1 - status::MAX_BYTES;
/// The first bytes of the file; a new form gets a new tag.
const TAG: &[u8; 17] = b"foundation/name/1";
/// The length of the tag, the length of the name, and the name with its padding.
const BODY: usize = TAG.len() + 1 + Name::MAX_BYTES;
/// The length of the file: the body and its CRC32C.
pub(crate) const LEN: usize = BODY + 4;
const _: () = assert!(
    Name::MAX_BYTES <= u8::MAX as usize,
    "one byte holds the length"
);
/// The pool of the read of the founding. The claim reads it before the shard's buffer
/// opens, and [`stored`] on no shard.
const POOL: block::Config = match block::Config::new(64 << 10) {
    Ok(config) => config,
    Err(_) => panic!("invariant: the budget fits"),
};

/// The name in the file `name` of `files`, or `None` when no node wrote one. Opens
/// the file to read only, so it makes nothing and waits for no lock.
///
/// # Errors
///
/// [`Error::Name`] for a file that a node did not write, and [`Error::Directory`] for
/// a file call that fails.
pub(crate) async fn read(files: &Files) -> Result<Option<Name>, Error> {
    let held = sector::read(files, Path::new(FILE), TAG).await;
    decode(&held.map_err(Error::Directory)?)
}

/// The stored name of the node of `files`: the one in the file `name`, else the one in
/// the node's card in the founding that the data directory stores, else `None` with
/// no founding. Opens each file to read only, so it makes nothing and waits for no
/// lock.
///
/// # Errors
///
/// With no file `name`: [`Error::Mesh`] for a founding that does not read,
/// [`Error::Key`] for a file `node.key` that a node did not write, and
/// [`Error::NoCard`] for a founding with no card of the key in `node.key` or with no
/// `node.key`. [`Error::Name`] for a file `name` that a node did not write, and
/// [`Error::Directory`] for a file call that fails.
pub(crate) async fn stored(files: &Files) -> Result<Option<Name>, Error> {
    match source(files).await? {
        Source::File(stored) | Source::Card(stored) => Ok(Some(stored)),
        Source::Nothing => Ok(None),
    }
}

/// Where the stored name is.
enum Source {
    File(Name),
    Card(Name),
    /// No file `name` and no founding.
    Nothing,
}

/// The source of the stored name of the node of `files`, as [`stored`] gives it.
async fn source(files: &Files) -> Result<Source, Error> {
    if let Some(stored) = read(files).await? {
        return Ok(Source::File(stored));
    }
    // The founding first: a first start makes `node.key` durable before it, so a read
    // that sees the founding also sees the key. The key also with no founding, so that
    // `Error::Key` comes before `Unnamed`.
    let founding = founding(files).await?;
    let key = identity::key(files).await?;
    let Some(founding) = founding else {
        return Ok(Source::Nothing);
    };
    let key = key.ok_or(Error::NoCard)?;
    let mut members = founding.members.into_iter();
    let member = members.find(|member| member.card.key() == key);
    let member = member.ok_or(Error::NoCard)?;
    Ok(Source::Card(member.card.card().name.clone()))
}

/// The founding that the data directory of `files` stores. Makes the pool of the read,
/// whose reservation is over 2 MiB, only when the mesh's directory is there.
async fn founding(files: &Files) -> Result<Option<Founding>, Error> {
    if !directory::meshed(files).await? {
        return Ok(None);
    }
    let pool = Rc::new(block::Pool::heap(POOL));
    mesh::founding(files, &directory::mesh(), pool)
        .await
        .map_err(Error::Mesh)
}

/// Writes `name` to the file `name` of `files` and makes it durable, when the file is
/// not there and the stored name ([`stored`]) is `name` or `None`. Call it under the
/// lock of the data directory, after the sync of the directory that the claim makes: a
/// failed sync of an earlier start can leave a name that a read sees but a crash loses.
///
/// # Errors
///
/// [`Error::Renamed`] when the stored name ([`stored`]) is another name, and each
/// error of [`stored`].
pub(crate) async fn keep(files: &Files, name: &Name) -> Result<(), Error> {
    match source(files).await? {
        Source::File(stored) => return same(stored, name),
        Source::Card(stored) => same(stored, name)?,
        Source::Nothing => {}
    }
    sector::publish(files, Path::new(FILE), &encode(name))
        .await
        .map_err(Error::Directory)
}

/// [`Error::Renamed`] when `stored` is not `given`.
fn same(stored: Name, given: &Name) -> Result<(), Error> {
    if stored == *given {
        return Ok(());
    }
    let given = given.clone();
    Err(Error::Renamed { stored, given })
}

/// The bytes of the file that holds `name`.
fn encode(name: &Name) -> [u8; LEN] {
    let name = name.as_str().as_bytes();
    let mut bytes = [0; LEN];
    bytes[..TAG.len()].copy_from_slice(TAG);
    bytes[TAG.len()] = u8::try_from(name.len()).expect("invariant: a name fits a u8");
    bytes[TAG.len() + 1..][..name.len()].copy_from_slice(name);
    sector::checksum(&mut bytes);
    bytes
}

/// The name that `held` holds, `None` for nothing, or [`Error::Name`] for bytes that
/// [`encode`] does not give.
fn decode(held: &Held<LEN>) -> Result<Option<Name>, Error> {
    let bytes = match held {
        Held::Nothing => return Ok(None),
        Held::Written(bytes) => bytes,
        Held::Foreign => return Err(Error::Name),
    };
    let len = usize::from(bytes[TAG.len()]);
    let name = &bytes[TAG.len() + 1..][..len];
    let name = std::str::from_utf8(name)
        .ok()
        .and_then(|name| name.parse::<Name>().ok())
        .ok_or(Error::Name)?;
    // Also refuses padding that is not zero, so each name has one form.
    if encode(&name) == *bytes {
        Ok(Some(name))
    } else {
        Err(Error::Name)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn writes_the_tag_the_length_the_name_and_the_checksum() {
        let bytes = encode(&"site_a.edge".parse().unwrap());
        assert_eq!(&bytes[..17], b"foundation/name/1");
        assert_eq!(bytes[17], 11);
        assert_eq!(&bytes[18..29], b"site_a.edge");
        assert_eq!(bytes[29..273], [0; 244]);
        assert_eq!(
            bytes[273..],
            crc32c::crc32c(&bytes[..273]).to_le_bytes(),
            "little-endian"
        );
    }

    #[test]
    fn a_name_of_the_most_bytes_fills_the_body() {
        let name: Name = "a".repeat(Name::MAX_BYTES).parse().unwrap();
        let bytes = encode(&name);
        assert_eq!(bytes[17], 255);
        assert!(sector::written(&bytes, TAG));
        assert_eq!(decode(&Held::Written(bytes)).unwrap(), Some(name));
    }

    #[test]
    fn zero_bytes_are_not_a_name() {
        assert!(!sector::written(&[0; LEN], TAG));
        assert!(matches!(decode(&Held::Foreign), Err(Error::Name)));
    }

    proptest! {
        /// Each byte that changes in the file of a name makes the file one that a
        /// node did not write.
        #[test]
        fn a_changed_byte_is_not_a_name(
            name in "[a-z][a-z0-9_]{0,20}(\\.[a-z0-9_-]{1,20}){0,3}",
            at in 0..LEN,
            flip in 1..=u8::MAX,
        ) {
            let mut bytes = encode(&name.parse().unwrap());
            prop_assert!(sector::written(&bytes, TAG));
            let decoded = decode(&Held::Written(bytes)).unwrap();
            prop_assert_eq!(decoded, Some(name.parse().unwrap()));
            bytes[at] ^= flip;
            prop_assert!(!sector::written(&bytes, TAG));
        }

        /// Bytes that no encode gives are not a name.
        #[test]
        fn other_bytes_are_not_a_name(bytes in prop::array::uniform32(any::<u8>())) {
            let mut file = [0; LEN];
            file[..32].copy_from_slice(&bytes);
            prop_assert!(!sector::written(&file, TAG));
        }
    }
}
