//! The read of a spec pointer from a file name never panics, and a name it reads is
//! the name that its pointer gives.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|name: &[u8]| {
    if let Some(out) = mesh::testing::round_trip_spec_name(name) {
        assert_eq!(out, name, "the name changed");
    }
});
