//! A write of `os::paths` that the OS refuses midway. The file size limit and what
//! the process does at `SIGXFSZ` are values of the process, so this test runs in a
//! test binary of its own, with this one test only: the harness runs the tests of a
//! binary on threads of one process.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))]

#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary only makes a scratch directory")]
mod disk;

use std::fs;
use std::os::unix::fs::symlink;

use disk::{Scratch, mode};
use env::paths::{Error, Found};
use rustix::io::Errno;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

/// Makes `dir` take new files and refuse each remove and rename, or takes that back.
/// On Linux only root can make such a directory, so there
/// `root::paths::a_write_past_the_size_limit_gives_the_error_of_a_remove_that_fails`
/// has the error of a remove that fails.
#[cfg(target_os = "macos")]
fn append_only(dir: &std::path::Path, on: bool) {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    let flags = if on { libc::UF_APPEND } else { 0 };
    // SAFETY: `path` ends in a NUL and lives past the call.
    #[expect(unsafe_code, reason = "`rustix` has no `chflags`")]
    let set = unsafe { libc::chflags(path.as_ptr(), flags) };
    assert_eq!(set, 0, "{}", std::io::Error::last_os_error());
}

#[test]
fn a_write_past_the_size_limit_removes_its_file_and_leaves_a_part_in_place() {
    let scratch = Scratch::new();
    let [behind, link, file] =
        ["behind", "link", "plant"].map(|name| scratch.0.join(name));
    for path in [&behind, &file] {
        fs::write(path, b"old plan").unwrap();
    }
    symlink(&behind, &link).unwrap();
    let kept = scratch.0.join("kept");
    fs::create_dir(&kept).unwrap();
    let open = kept.join("plant");
    fs::write(&open, b"old plan").unwrap();
    #[cfg(target_os = "macos")]
    append_only(&kept, true);

    // SAFETY: `SIG_IGN` is a valid disposition of `SIGXFSZ`.
    #[expect(unsafe_code, reason = "the write must get `EFBIG`, not the signal")]
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
    }
    let limit = getrlimit(Resource::Fsize);
    let small = Rlimit {
        current: Some(4),
        maximum: limit.maximum,
    };
    setrlimit(Resource::Fsize, small).unwrap();
    let paths = os::paths();
    let written = [&file, &link, &open].map(|path| paths.write(path, b"a new plan"));
    setrlimit(Resource::Fsize, limit).unwrap();
    #[cfg(target_os = "macos")]
    append_only(&kept, false);

    let large = Err(Error::Io {
        code: Errno::FBIG.raw_os_error(),
    });
    assert_eq!(written[..2], [large, large]);
    assert_eq!(paths.read(&file, 8), Ok(Found::File(b"old plan".to_vec())));
    assert_eq!(paths.read(&behind, 8), Ok(Found::File(b"a ne".to_vec())));
    let entries = vec![behind, kept.clone(), link, file];
    assert_eq!(
        paths.read(&scratch.0, 1 << 12),
        Ok(Found::Directory(entries))
    );
    // The remove of the hidden file fails on macOS, and the write gives its error.
    #[cfg(target_os = "macos")]
    let (refused, left) = (Errno::PERM.raw_os_error(), 1);
    #[cfg(target_os = "linux")]
    let (refused, left) = (Errno::FBIG.raw_os_error(), 0);
    assert_eq!(written[2], Err(Error::Io { code: refused }));
    let Ok(Found::Directory(mut hidden)) = paths.read(&kept, 1 << 12) else {
        panic!("`kept` is a directory");
    };
    hidden.retain(|entry| *entry != open);
    assert_eq!(hidden.len(), left, "{hidden:?}");
    // Only this user reads the part of the bytes that stays.
    assert!(hidden.iter().all(|part| mode(part) == 0o600));
}
