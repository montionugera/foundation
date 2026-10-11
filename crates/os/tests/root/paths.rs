//! `os::paths` on a file that only root can make: one of another user, or with a
//! group that the user of the test is not in.

use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::process::Command;

use crate::disk::{Scratch, mode};
use crate::{check, groups, sudo};

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
