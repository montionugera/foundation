//! `node::sector::written` takes the bytes of `node.key` or `admin.key` exactly when
//! they have the tag of their file and the CRC32C, and the encode of their fields gives
//! them back.
//! `node::address::decode` gives an address exactly for the bytes that the encode of
//! `address` writes, with the tag and the CRC32C, and that address encodes to the same
//! bytes. It gives none for the zero bytes that a clean stop leaves.
//!
//! Input: the 100 bytes of `node.key`, the 54 bytes of `admin.key`, or the 99 bytes of
//! `address`, or the first 96, 50, or 95 bytes, to which the target appends their
//! CRC32C.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| node::fuzz::identity(data));
