//! The file `admin.key`: the private key of the first admin of the root region.
//!
//! The file is 54 bytes: the tag, the Ed25519 private key, and the CRC32C of those 50
//! bytes (little-endian), in one sector.

use types::ed25519::PrivateKey;

/// The name of the file.
pub const FILE: &str = "admin.key";
/// The first bytes of the file; a new form gets a new tag.
pub const TAG: &[u8; 18] = b"foundation/admin/1";
/// The length of the tag and the private key.
const BODY: usize = 50;
/// The length of the file: the body and its CRC32C.
pub const LEN: usize = BODY + 4;

/// The bytes of the file that holds `private_key`, which [`crate::publish`] makes.
#[must_use]
pub fn encode(private_key: &PrivateKey) -> [u8; LEN] {
    let mut bytes = [0; LEN];
    bytes[..TAG.len()].copy_from_slice(TAG);
    bytes[TAG.len()..BODY].copy_from_slice(&private_key.0);
    crate::checksum(&mut bytes);
    bytes
}

/// The private key in `bytes`, or `None` for bytes that [`encode`] does not write.
#[must_use]
pub fn decode(bytes: &[u8; LEN]) -> Option<PrivateKey> {
    crate::written(bytes, TAG).then(|| field(bytes))
}

/// The private key in `bytes`, whatever its tag and checksum.
fn field(bytes: &[u8; LEN]) -> PrivateKey {
    PrivateKey(
        bytes[TAG.len()..BODY]
            .try_into()
            .expect("invariant: 32 bytes"),
    )
}

/// Checks that `crate::written` takes `bytes` exactly when `encode` of their fields
/// gives them back.
///
/// # Panics
///
/// When the check fails.
#[cfg(any(test, feature = "sim"))]
pub fn check(bytes: &[u8; LEN]) {
    crate::check(bytes, TAG, |bytes| Some(encode(&field(bytes))));
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
        assert!(!crate::written(&[0; LEN], TAG));
    }

    proptest! {
        #[test]
        fn decodes_what_it_encodes(private_key in any::<[u8; 32]>()) {
            let bytes = encode(&PrivateKey(private_key));
            prop_assert!(crate::written(&bytes, TAG));
            prop_assert_eq!(field(&bytes).0, private_key);
        }

        #[test]
        fn refuses_each_one_bit_change(
            private_key in any::<[u8; 32]>(),
            bit in 0..LEN * 8,
        ) {
            let mut bytes = encode(&PrivateKey(private_key));
            bytes[bit / 8] ^= 1 << (bit % 8);
            prop_assert!(!crate::written(&bytes, TAG));
        }

        #[test]
        fn refuses_each_one_bit_change_of_the_tag_with_its_checksum(
            private_key in any::<[u8; 32]>(),
            bit in 0..TAG.len() * 8,
        ) {
            let mut bytes = encode(&PrivateKey(private_key));
            check(&bytes);
            bytes[bit / 8] ^= 1 << (bit % 8);
            crate::checksum(&mut bytes);
            prop_assert!(!crate::written(&bytes, TAG));
        }

        #[test]
        fn checks_any_bytes(bytes in any::<[u8; LEN]>()) {
            check(&bytes);
        }

        #[test]
        fn checks_any_body_with_its_checksum(mut bytes in any::<[u8; LEN]>()) {
            crate::checksum(&mut bytes);
            check(&bytes);
        }
    }
}
