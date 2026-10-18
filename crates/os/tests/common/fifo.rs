//! A FIFO, and a stall of the I/O thread of a disk in the open of one.

use std::path::Path;

use env::files::{Files, Mode};

/// Runs `start` while the I/O thread of `files` blocks in the open of a FIFO, so each
/// call that `start` polls once waits in the queue, and then frees the thread.
pub(crate) async fn stalled<T>(
    files: &Files,
    data: &Path,
    start: impl FnOnce(&mut std::task::Context<'_>) -> T,
) -> T {
    make(&data.join("p"));
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    let mut blocker = Box::pin(files.open(Path::new("p"), Mode::Read));
    assert!(blocker.as_mut().poll(&mut context).is_pending());
    let started = start(&mut context);
    let writer = rustix::fs::open(
        data.join("p"),
        rustix::fs::OFlags::WRONLY,
        rustix::fs::Mode::empty(),
    );
    drop(blocker.await.unwrap());
    drop(writer.unwrap());
    std::fs::remove_file(data.join("p")).unwrap();
    started
}

/// Makes a FIFO at `path`.
pub(crate) fn make(path: &Path) {
    #[cfg(target_os = "linux")]
    {
        let mode = rustix::fs::Mode::from_raw_mode(0o600);
        rustix::fs::mkfifoat(rustix::fs::CWD, path, mode).unwrap();
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `path` ends in a NUL and lives past the call.
        #[expect(unsafe_code, reason = "`rustix` has no `mkfifoat` on macOS")]
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "{}", std::io::Error::last_os_error());
    }
}
