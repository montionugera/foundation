//! A program moves the working directory of the process away and makes it again. The
//! test changes the working directory of the process, so it runs in a test binary of
//! its own, with this one test only.

// Lets Clippy treat the test as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::Path;
use std::slice;

use env::paths::Error;

/// The process stays on the moved directory, so a relative path resolves in it, and a
/// write there is in the folder that `within` took.
#[test]
fn a_relative_path_in_a_moved_working_directory_is_replaced() {
    let base =
        std::env::temp_dir().join(format!("moved-working-{}", std::process::id()));
    fs::create_dir(&base).unwrap();
    let base = fs::canonicalize(&base).unwrap();
    let work = base.join("work");
    fs::create_dir(&work).unwrap();
    std::env::set_current_dir(&work).unwrap();
    let paths = os::paths();
    let within = paths.within(slice::from_ref(&work), &[]).unwrap();
    fs::rename(&work, base.join("old")).unwrap();
    fs::create_dir(&work).unwrap();
    let resolved = paths.resolve(Path::new("."));
    let written = within.write(Path::new("out.plan"), b"x");
    fs::remove_dir_all(&base).unwrap();
    assert_eq!(
        (resolved, written),
        (Ok(base.join("old")), Err(Error::Replaced))
    );
}
