//! `node::identity::decode`, `node::admin::decode`, and `node::address::decode` each
//! give a value exactly for the bytes that their file can hold, `node.key`,
//! `admin.key`, or `address`, with the tag and the CRC32C, and that value encodes to
//! the same bytes.
//!
//! Input: the 100 bytes of `node.key`, the 54 bytes of `admin.key`, or the 99 bytes of
//! `address`, or the first 96, 50, or 95 bytes, to which the target appends their
//! CRC32C.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| node::fuzz::identity(data));
