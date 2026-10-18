//! A relative hidden path after the working directory of the process goes. The test
//! changes the working directory of the process, so it runs in a test binary of its
//! own, with this one test only.

// Lets Clippy treat the test as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))] // #2956

use std::fs;
use std::path::PathBuf;
use std::slice;

use env::paths::Error;

/// A relative hidden path starts at the working directory of `within`, so it stays
/// hidden when that directory goes.
#[test]
fn a_relative_hidden_path_stays_after_the_working_directory_goes() {
    let base =
        std::env::temp_dir().join(format!("hidden-working-{}", std::process::id()));
    fs::create_dir(&base).unwrap();
    let base = fs::canonicalize(&base).unwrap();
    let work = base.join("w");
    fs::create_dir_all(work.join("sub")).unwrap();
    fs::write(work.join("secret"), b"x").unwrap();
    std::env::set_current_dir(work.join("sub")).unwrap();
    let paths = os::paths();
    let hidden = [PathBuf::from("../secret")];
    let within = paths.within(slice::from_ref(&work), &hidden).unwrap();
    fs::remove_dir(work.join("sub")).unwrap();
    let read = within.read(&work.join("secret"), 1);
    fs::remove_dir_all(&base).unwrap();
    assert_eq!(read, Err(Error::Hidden));
}
