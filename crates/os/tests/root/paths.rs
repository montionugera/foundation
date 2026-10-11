//! `os::paths` on what only root can make: a file of another user, a file with a
//! group that the user of the test is not in, and a directory that refuses a remove.

use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::process::Command;

use env::paths::Error;
use rustix::io::Errno;
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use crate::disk::{Scratch, mode};
use crate::{Small, check, groups, sudo};

/// The user and the group `nobody`: not the user of the test, and not a group of it.
const NOBODY: u32 = 65534;

#[test]
fn a_write_closes_the_file_to_a_group_that_the_user_is_not_in() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    let made = fs::metadata(&path).unwrap().gid();
    let groups = rustix::process::getgroups().unwrap();
    assert!(groups.iter().all(|group| group.as_raw() != NOBODY));
    fs::set_permissions(&path, Permissions::from_mode(0o664)).unwrap();
    check(sudo("chgrp").arg(NOBODY.to_string()).arg(&path));
    os::paths().write(&path, b"new").unwrap();
    let new = fs::metadata(&path).unwrap();
    assert_eq!((new.gid(), mode(&path)), (made, 0o604));
}

#[test]
fn a_write_keeps_a_group_of_the_user_on_a_file_of_another_user() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    let made = fs::metadata(&path).unwrap();
    let other = groups::other(made.gid());
    fs::set_permissions(&path, Permissions::from_mode(0o660)).unwrap();
    check(sudo("chown").arg(format!("{NOBODY}:{other}")).arg(&path));
    os::paths().write(&path, b"new").unwrap();
    let new = fs::metadata(&path).unwrap();
    assert_eq!(
        (new.uid(), new.gid(), mode(&path)),
        (made.uid(), other, 0o660)
    );
}

#[test]
fn a_write_as_root_keeps_the_owner_the_group_and_the_bits() {
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
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    chown(&path, Some(NOBODY), Some(NOBODY)).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();
    os::paths().write(&path, b"new").unwrap();
    let new = fs::metadata(&path).unwrap();
    assert_eq!((new.uid(), new.gid(), mode(&path)), (NOBODY, NOBODY, 0o640));
}

/// A directory with the append-only attribute takes new files and refuses each
/// remove. The size limit is a value of the process, so the write runs alone in a
/// child, which a limit of its own marks.
#[test]
fn a_write_past_the_size_limit_gives_the_error_of_a_remove_that_fails() {
    const MARK: u64 = 1 << 30;
    let limit = getrlimit(Resource::Fsize);
    if limit.current != Some(MARK) {
        let thread = std::thread::current();
        let test = thread.name().expect("invariant: libtest names the thread");
        // An ignored `SIGXFSZ` stays ignored across `exec`, so it does not end the
        // child.
        let script =
            r#"trap '' XFSZ && ulimit -f "$0" && exec "$1" --exact "$2" --nocapture"#;
        let mut child = Command::new("sh");
        child.args(["-c", script]).arg((MARK / 512).to_string());
        check(child.arg(std::env::current_exe().unwrap()).arg(test));
        return;
    }
    let disk = Small::new();
    let kept = disk.mount().join("kept");
    fs::create_dir(&kept).unwrap();
    let path = kept.join("plant");
    fs::write(&path, b"old plan").unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap();
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
    // The part of the bytes that stays does not have the bits of the old file yet.
    let modes: Vec<_> = hidden.iter().map(|part| mode(part)).collect();
    assert_eq!(modes, [0o600]);
}
