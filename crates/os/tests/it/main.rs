//! Production-path tests of `os` on the real operating system.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2956
#[path = "../../../env/tests/common/paths.rs"]
mod cases;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #682
mod clock;
mod common;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #2956
#[path = "../common/mod.rs"]
mod disk;
mod entropy;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #2956
mod files;
#[cfg(target_os = "linux")]
#[path = "../common/fsize.rs"]
mod fsize;
#[cfg(target_os = "linux")]
mod kept;
mod log;
#[cfg(target_os = "linux")]
mod memory;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #2955
mod net;
#[path = "../common/panics.rs"]
mod panics;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #2956
mod paths;
#[path = "../../../env/tests/common/log.rs"]
mod recorded;
#[cfg(target_os = "linux")]
#[path = "../common/seccomp.rs"]
mod seccomp;
mod shards;
mod threads;
#[cfg(any(target_os = "linux", target_os = "macos"))] // #682
mod wall;
