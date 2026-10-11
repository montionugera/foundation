//! `os::paths` on what only root can do: a write over a file of another user, and a
//! directory that refuses a remove.

use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::process::Command;

use env::paths::Error;
use rustix::io::Errno;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use crate::disk::{Scratch, mode};
use crate::{Small, check, fsize, sudo};

/// The user and the group `nobody`: not root, and not the group of a file of root.
const NOBODY: u32 = 65534;

#[test]
fn a_write_as_root_takes_no_owner_group_or_bits_of_the_file_it_replaces() {
    if !rustix::process::geteuid().is_root() {
        let thread = std::thread::current();
        let test = thread.name().expect("invariant: libtest names the thread");
        // The write must run as root, so this test runs again, alone, in a child.
        let mut child = Command::new("sudo");
        child.arg("-n").arg(std::env::current_exe().unwrap());
        check(child.args(["--exact", test, "--nocapture"]));
        return;
    }
    let scratch = Scratch::new();
    let (path, made) = (scratch.0.join("plant.hcl"), scratch.0.join("made.hcl"));
    fs::write(&path, b"old").unwrap();
    chown(&path, Some(NOBODY), Some(NOBODY)).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();
    let paths = os::paths();
    let [new, made] = [&path, &made].map(|path| {
        paths.write(path, b"new").unwrap();
        let file = fs::metadata(path).unwrap();
        (file.uid(), file.gid(), mode(path))
    });
    assert_eq!((new, made.2), (made, 0o600));
}

/// A directory with the append-only attribute takes new files and refuses each
/// remove. The size limit is a value of the process, so the write runs alone in a
/// child, which a limit of its own marks.
#[test]
fn a_write_past_the_size_limit_gives_the_error_of_a_remove_that_fails() {
    fsize::under(1 << 30, || {
        let limit = getrlimit(Resource::Fsize);
        let disk = Small::new();
        let kept = disk.mount().join("kept");
        fs::create_dir(&kept).unwrap();
        let path = kept.join("plant");
        fs::write(&path, b"old plan").unwrap();
        check(sudo("chattr").arg("+a").arg(&kept));
        let small = Rlimit {
            current: Some(4),
            maximum: limit.maximum,
        };
        setrlimit(Resource::Fsize, small).unwrap();
        let written = os::paths().write(&path, b"a new plan");
        setrlimit(Resource::Fsize, limit).unwrap();
        let code = Errno::PERM.raw_os_error();
        assert_eq!(written, Err(Error::Io { code }));
        let entries = fs::read_dir(&kept)
            .unwrap()
            .map(|entry| entry.unwrap().path());
        let hidden: Vec<_> = entries.filter(|entry| *entry != path).collect();
        // Only this user reads the part of the bytes that stays.
        let modes: Vec<_> = hidden.iter().map(|part| mode(part)).collect();
        assert_eq!(modes, [0o600]);
    });
}
