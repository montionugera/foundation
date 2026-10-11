//! `node::sector::written` takes the bytes of `node.key` or `admin.key` exactly when
//! they have the tag of their file and the CRC32C, and the encode of their fields gives
//! them back.
//!
//! Input: the 100 bytes of `node.key` or the 54 bytes of `admin.key`, or the first 96
//! or 50 bytes, to which the target appends their CRC32C.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| node::fuzz::identity(data));
