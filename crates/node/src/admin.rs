//! The private key of the first admin of the root region, in the file `admin.key` of
//! the data directory.
//!
//! The file is 54 bytes: the tag, the Ed25519 private key, and the CRC32C of those 50
//! bytes (little-endian), in one sector ([`crate::sector`]).

use std::path::Path;

use env::files::Files;
use types::ed25519::PrivateKey;

use crate::Error;
use crate::sector::{self, Held};

/// The name of the file.
pub(crate) const FILE: &str = "admin.key";
/// The first bytes of the file; a new form gets a new tag.
const TAG: &[u8; 18] = b"foundation/admin/1";
/// The length of the tag and the private key.
const BODY: usize = 50;
/// The length of the file: the body and its CRC32C.
pub(crate) const LEN: usize = BODY + 4;

/// The private key in `admin.key` of `files`. When the file is not there, makes a new
/// one from `entropy` and makes it durable. Call it under the lock of the data
/// directory, after the sync of the directory that the claim makes: a failed sync of
/// an earlier start can leave a key that a read sees but a crash loses.
///
/// # Errors
///
/// [`Error::Admin`] for a file that a node did not write, and [`Error::Directory`] for
/// a file call that fails.
pub(crate) async fn load(
    files: &Files,
    entropy: &env::entropy::Entropy,
) -> Result<PrivateKey, Error> {
    let held = sector::read(files, Path::new(FILE), TAG).await;
    match held.map_err(Error::Directory)? {
        Held::Nothing => {
            let mut bytes = [0; 32];
            entropy.fill(&mut bytes);
            let private_key = PrivateKey(bytes);
            sector::publish(files, Path::new(FILE), &encode(&private_key))
                .await
                .map_err(Error::Directory)?;
            Ok(private_key)
        }
        Held::Written(bytes) => Ok(field(&bytes)),
        Held::Foreign => Err(Error::Admin),
    }
}

/// The private key in `admin.key` of `files`, or `None` when no node wrote the file.
/// Opens it to read only, so it makes nothing and waits for no lock.
///
/// # Errors
///
/// The error of each file call that fails, except [`env::files::Error::NotFound`].
pub(crate) async fn read(
    files: &Files,
) -> Result<Option<PrivateKey>, env::files::Error> {
    let held = sector::read::<LEN>(files, Path::new(FILE), TAG).await?;
    Ok(match held {
        Held::Written(bytes) => Some(field(&bytes)),
        Held::Nothing | Held::Foreign => None,
    })
}

/// The bytes of the file that holds `private_key`.
fn encode(private_key: &PrivateKey) -> [u8; LEN] {
    let mut bytes = [0; LEN];
    bytes[..TAG.len()].copy_from_slice(TAG);
    bytes[TAG.len()..BODY].copy_from_slice(&private_key.0);
    sector::checksum(&mut bytes);
    bytes
}

/// The private key in `bytes`, whatever its tag and checksum.
fn field(bytes: &[u8; LEN]) -> PrivateKey {
    PrivateKey(
        bytes[TAG.len()..BODY]
            .try_into()
            .expect("invariant: 32 bytes"),
    )
}

/// Checks that `sector::written` takes `bytes` exactly when `encode` of their fields
/// gives them back.
///
/// # Panics
///
/// When the check fails.
#[cfg(any(test, feature = "sim"))]
pub(crate) fn check(bytes: &[u8; LEN]) {
    sector::check(bytes, TAG, |bytes| Some(encode(&field(bytes))));
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn writes_the_tag_the_key_and_the_checksum() {
        let bytes = encode(&PrivateKey([7; 32]));
        assert_eq!(&bytes[..18], b"foundation/admin/1");
        assert_eq!(bytes[18..50], [7; 32]);
        assert_eq!(
            bytes[50..],
            crc32c::crc32c(&bytes[..50]).to_le_bytes(),
            "little-endian"
        );
    }

    #[test]
    fn refuses_zero_bytes() {
        assert!(!sector::written(&[0; LEN], TAG));
    }

    proptest! {
        #[test]
        fn decodes_what_it_encodes(private_key in any::<[u8; 32]>()) {
            let bytes = encode(&PrivateKey(private_key));
            prop_assert!(sector::written(&bytes, TAG));
            prop_assert_eq!(field(&bytes).0, private_key);
        }

        #[test]
        fn refuses_each_one_bit_change(
            private_key in any::<[u8; 32]>(),
            bit in 0..LEN * 8,
        ) {
            let mut bytes = encode(&PrivateKey(private_key));
            bytes[bit / 8] ^= 1 << (bit % 8);
            prop_assert!(!sector::written(&bytes, TAG));
        }

        #[test]
        fn refuses_each_one_bit_change_of_the_tag_with_its_checksum(
            private_key in any::<[u8; 32]>(),
            bit in 0..TAG.len() * 8,
        ) {
            let mut bytes = encode(&PrivateKey(private_key));
            check(&bytes);
            bytes[bit / 8] ^= 1 << (bit % 8);
            sector::checksum(&mut bytes);
            prop_assert!(!sector::written(&bytes, TAG));
        }

        #[test]
        fn checks_any_bytes(bytes in any::<[u8; LEN]>()) {
            check(&bytes);
        }

        #[test]
        fn checks_any_body_with_its_checksum(mut bytes in any::<[u8; LEN]>()) {
            sector::checksum(&mut bytes);
            check(&bytes);
        }
    }
}
