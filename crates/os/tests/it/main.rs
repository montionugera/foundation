//! Production-path tests of `os` on the real operating system.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod clock;
mod common;
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[path = "../common/mod.rs"]
mod disk;
mod entropy;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod files;
#[cfg(target_os = "linux")]
mod kept;
#[cfg(target_os = "linux")]
mod memory;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod net;
#[path = "../common/panics.rs"]
mod panics;
#[cfg(target_os = "linux")]
#[path = "../common/seccomp.rs"]
mod seccomp;
mod shards;
mod threads;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod wall;

/// A child holds the file locks of each other test of this binary until its `exec`, so
/// a test here that starts one makes them give `Busy`. A test that starts a child goes
/// in a binary of its own.
#[test]
fn no_test_of_this_binary_starts_a_child() {
    use std::path::Path;

    fn sources(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    let tests = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut found = ["mod.rs", "panics.rs", "seccomp.rs"]
        .map(|name| tests.join("common").join(name))
        .to_vec();
    sources(&tests.join("it"), &mut found);
    // Split, so this file does not name them.
    let starts = [
        concat!("Command", "::new"),
        concat!("process", "::Command"),
        concat!("pre", "_exec"),
        concat!("current", "_exe"),
    ];
    for path in found {
        let text = std::fs::read_to_string(&path).unwrap();
        for start in starts {
            assert!(!text.contains(start), "{} names {start}", path.display());
        }
    }
}
