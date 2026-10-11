//! A file size limit for one test on Linux. The limit is a value of the process, and
//! the harness runs the tests of a binary on threads of one process. The `sh` of
//! macOS counts the limit in blocks of 1024 bytes, not 512.

use std::process::Command;

use rustix::process::{Resource, getrlimit};

/// Runs `test` in a child of this test binary that runs only the test of this thread,
/// has a file size limit of `bytes`, a multiple of 512, and ignores `SIGXFSZ`. Panics
/// with the error output of a child that fails.
pub(crate) fn under(bytes: u64, test: impl FnOnce()) {
    const CHILD: &str = "OS_FSIZE_CHILD";
    #[expect(
        clippy::disallowed_methods,
        reason = "the parent sets it for the child"
    )]
    if std::env::var_os(CHILD).is_some() {
        // `ulimit -f` counts blocks of 512 bytes.
        assert_eq!(getrlimit(Resource::Fsize).current, Some(bytes));
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
        .env(CHILD, "1")
        .output()
        .unwrap();
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{error}");
}
