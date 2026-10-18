//! Only Unix: `os` has no `paths` driver on Windows.
//!
//! A walk of `os::paths` with one free descriptor, so the open of each directory after
//! the root gives `EMFILE`. The descriptor table is state of the process.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))]

#[path = "common/descriptors.rs"]
mod descriptors;
#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary only makes a scratch directory")]
mod disk;

use std::fs;

use disk::Scratch;
use env::paths::Error;
use rustix::io::Errno;

#[test]
fn a_walk_gives_the_error_of_an_open_that_fails_for_a_descriptor() {
    let scratch = Scratch::new();
    let path = scratch.0.join("a/f");
    fs::create_dir(scratch.0.join("a")).unwrap();
    fs::write(&path, b"plan").unwrap();
    descriptors::limit();
    let mut held = descriptors::take_each();
    held.pop();
    let resolved = os::paths().resolve(&path);
    drop(held);
    let full = Err(Error::Io {
        code: Errno::MFILE.raw_os_error(),
    });
    assert_eq!(resolved, full);
}
