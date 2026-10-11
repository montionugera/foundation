//! A file size limit for one test on Linux. The limit is a value of the process, and
//! the harness runs the tests of a binary on threads of one process. The `sh` of
//! macOS counts the limit in blocks of 1024 bytes, not 512.

use std::process::Command;

use rustix::process::{Resource, getrlimit};

/// Runs `test` in a process whose file size limit is `bytes`, a multiple of 512, and
/// that ignores `SIGXFSZ`. A process with another limit runs the test of this thread
/// again, alone, in a child that has the limit, and panics with the error output of
/// a child that fails.
pub(crate) fn under(bytes: u64, test: impl FnOnce()) {
    // `ulimit -f` counts blocks of 512 bytes. With another limit, each child would
    // start a child.
    assert_eq!(bytes % 512, 0);
    if getrlimit(Resource::Fsize).current == Some(bytes) {
        return test();
    }
    let thread = std::thread::current();
    let name = thread.name().expect("invariant: libtest names the thread");
    // An ignored `SIGXFSZ` stays ignored across `exec`, so it does not end the child.
    let script =
        r#"trap '' XFSZ && ulimit -f "$0" && exec "$1" --exact "$2" --nocapture"#;
    let output = Command::new("sh")
        .args(["-c", script])
        .arg((bytes / 512).to_string())
        .arg(std::env::current_exe().unwrap())
        .arg(name)
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{error}");
}
