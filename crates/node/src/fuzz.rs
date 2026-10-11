//! Fuzz entry points, for the fuzz targets only. Not a stable surface.

use crate::{address, admin, identity, sector};

/// Decodes `data` by its length, as the bytes of a file of the data directory: 100
/// bytes of `node.key`, 54 bytes of `admin.key`, or 99 bytes of `address`, each as
/// they are, or 96, 50, or 95 bytes with their CRC32C appended. Ignores any other
/// length.
///
/// # Panics
///
/// When a decode gives a value for bytes that its file never holds, refuses bytes
/// that its file can hold, or gives a value that does not encode to the bytes.
pub fn identity(data: &[u8]) {
    if let Some(bytes) = file(data) {
        identity::check(&bytes);
    }
    if let Some(bytes) = file(data) {
        admin::check(&bytes);
    }
    if let Some(bytes) = file(data) {
        address::check(&bytes);
    }
}

/// `data` as the `N` bytes of a file: as they are, or with their CRC32C appended.
fn file<const N: usize>(data: &[u8]) -> Option<[u8; N]> {
    <[u8; N]>::try_from(data)
        .ok()
        .or_else(|| sector::summed(data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_its_bytes_or_its_body_with_the_checksum() {
        assert_eq!(file::<4>(&[1, 2, 3, 4]), Some([1, 2, 3, 4]));
        let mut bytes = [7; 54];
        sector::checksum(&mut bytes);
        assert_eq!(file::<54>(&bytes[..50]), Some(bytes));
        assert_eq!(file::<54>(&bytes[..53]), None);
    }
}
