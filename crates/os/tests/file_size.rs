//! A create past the file size limit of the process. The limit and the action of
//! `SIGXFSZ` are on the whole process, so this test runs in a test binary of its own.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(target_os = "linux")]

#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary uses some of the helpers")]
mod disk;

use std::path::Path;

use disk::{Scratch, files};
use env::files::{Error, Mode, Operation};
use rustix::process::{Resource, getrlimit, setrlimit};

/// Sets the action of `SIGXFSZ` to `SIG_IGN`, so a write past the limit fails with
/// `EFBIG` and does not end the process.
#[expect(unsafe_code, reason = "a signal action is an OS call")]
fn ignore_sigxfsz() {
    // SAFETY: `SIG_IGN` runs no code in the process.
    let old = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    assert_ne!(old, libc::SIG_ERR, "{}", std::io::Error::last_os_error());
}

#[test]
fn a_create_past_the_file_size_limit_leaves_no_file() {
    const LIMIT: u64 = 1 << 20;
    ignore_sigxfsz();
    let mut limit = getrlimit(Resource::Fsize);
    limit.current = Some(LIMIT);
    setrlimit(Resource::Fsize, limit).unwrap();
    let scratch = Scratch::new();
    let (files, thread) = files(&scratch.0, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mode = Mode::Create { len: 8 * LIMIT };
        let found = files.open(Path::new("a"), mode).await.unwrap_err();
        let expected = Error::Io {
            path: "a".into(),
            operation: Operation::Open,
            code: libc::EFBIG,
        };
        assert_eq!(found, expected);
        let found = files.open(Path::new("a"), Mode::Write).await.unwrap_err();
        assert_eq!(found, Error::NotFound { path: "a".into() });
    });
    drop(files);
    thread.join().unwrap();
}
