//! The open of a mesh log directory never panics, an open of the files that it leaves
//! reads the same hard state and entries, and a third open leaves the files that the
//! second left.
//!
//! The target seals each input before the open: it writes the checks of each record,
//! so a change to a header or a body still reaches the rules of the scan.

#![no_main]
#![expect(clippy::disallowed_methods, reason = "fuzz_target! calls File::create")]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|bytes: &[u8]| {
    let mut dir = bytes.to_vec();
    mesh::testing::seal_log_dir(&mut dir);
    if let Ok(first) = mesh::testing::scan_log_dir(&dir) {
        let second = mesh::testing::scan_log_dir(&first.files)
            .expect("an open refused the files that an open left");
        assert_eq!(second.hard, first.hard, "the hard state changed");
        assert_eq!(second.entries, first.entries, "the entries changed");
        let third = mesh::testing::scan_log_dir(&second.files)
            .expect("an open refused the files that an open left");
        assert_eq!(third.files, second.files, "the files changed after two opens");
    }
});
