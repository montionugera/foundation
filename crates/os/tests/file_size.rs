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
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

/// Sets the action of `SIGXFSZ` to `SIG_IGN`, so a write past the limit fails with
/// `EFBIG` and does not end the process.
#[expect(unsafe_code, reason = "a signal action is an OS call")]
fn ignore_sigxfsz() {
    // SAFETY: `SIG_IGN` runs no code in the process.
    let old = unsafe { libc::signal(libc::SIGXFSZ, libc::SIG_IGN) };
    assert_ne!(old, libc::SIG_ERR, "{}", std::io::Error::last_os_error());
}

/// The test sets the limit only around its calls, and asserts after it restores the
/// limit, since the limit also caps the writes of libtest to its output.
#[test]
fn a_create_past_the_file_size_limit_leaves_no_file() {
    const LIMIT: u64 = 1 << 20;
    ignore_sigxfsz();
    let scratch = Scratch::new();
    let (files, thread) = files(&scratch.0, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let old = getrlimit(Resource::Fsize);
    let limit = Rlimit {
        current: Some(LIMIT),
        ..old
    };
    setrlimit(Resource::Fsize, limit).unwrap();
    let (created, opened) = runtime.block_on(async {
        let mode = Mode::Create { len: 8 * LIMIT };
        let created = files.open(Path::new("a"), mode).await.map(drop);
        let opened = files.open(Path::new("a"), Mode::Write).await.map(drop);
        (created, opened)
    });
    setrlimit(Resource::Fsize, old).unwrap();
    let expected = Error::Io {
        path: "a".into(),
        operation: Operation::Open,
        code: libc::EFBIG,
    };
    assert_eq!(created, Err(expected));
    assert_eq!(opened, Err(Error::NotFound { path: "a".into() }));
    let past = vec![0; usize::try_from(LIMIT).unwrap() + 1];
    std::fs::write(scratch.0.join("past"), past).unwrap();
    drop(files);
    thread.join().unwrap();
}
