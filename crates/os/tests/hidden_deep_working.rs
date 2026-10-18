//! A short relative hidden path from a working directory near the longest path of the
//! OS. The test changes the working directory of the process, so it runs in a test
//! binary of its own, with this one test only.

// Lets Clippy treat the test as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))] // #2956

use std::fs;
use std::path::{Path, PathBuf};
use std::slice;

use env::paths::Error;

/// The longest path that the OS takes, less its end NUL.
const MOST: usize = if cfg!(target_os = "macos") {
    1023
} else {
    4095
};

/// The host bounds the path as given, so `within` takes the 6 bytes of `s/../k`
/// where the path on the working directory is longer than the host takes.
#[test]
fn a_short_relative_hidden_path_from_a_deep_working_directory_hides() {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hidden-deep-working-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    let base = fs::canonicalize(&base).unwrap();
    let mut deep = base.clone();
    while deep.as_os_str().len() + 202 < MOST - 6 {
        deep.push("d".repeat(200));
        fs::create_dir(&deep).unwrap();
    }
    deep.push("e".repeat(MOST - 7 - deep.as_os_str().len()));
    fs::create_dir(&deep).unwrap();
    assert_eq!(deep.as_os_str().len() + "/s/../k".len(), MOST + 1);
    std::env::set_current_dir(&deep).unwrap();
    fs::create_dir("s").unwrap();
    fs::write("k", b"x").unwrap();
    let hidden = PathBuf::from("s/../k");
    let within = os::paths().within(slice::from_ref(&deep), slice::from_ref(&hidden));
    let read = within.map(|within| within.read(&deep.join("k"), 1));
    std::env::set_current_dir(&base).unwrap();
    fs::remove_file(deep.join("k")).unwrap();
    fs::remove_dir(deep.join("s")).unwrap();
    while deep != base {
        fs::remove_dir(&deep).unwrap();
        deep.pop();
    }
    fs::remove_dir(&base).unwrap();
    assert_eq!(read, Ok(Err(Error::Hidden)));
}
