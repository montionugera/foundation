//! A child holds a copy of each descriptor of this process from its fork. It holds
//! each one that is closed on exec, as each file of `os::files` is, until its exec, or
//! to its end.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]
#![cfg(any(target_os = "linux", target_os = "macos"))] // #2956

#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary uses some of the helpers")]
mod disk;

use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use disk::{Scratch, files};
use env::files::{Absolute, Mode};

#[test]
#[expect(unsafe_code, reason = "a hook before exec is an OS call")]
fn a_child_does_not_hold_the_lock_of_a_closed_file() {
    let scratch = Scratch::new();
    let (files, thread) = files(&scratch.0, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let (forked, forked_in) = std::io::pipe().unwrap();
    let (released_out, released) = std::io::pipe().unwrap();
    let a = Path::new("a");
    let file = runtime
        .block_on(files.open(a, Mode::Create { len: 0 }))
        .unwrap();
    let mut command = Command::new("true");
    // SAFETY: the hook only reads and writes pipes, which is safe after a fork.
    unsafe {
        command.pre_exec(move || {
            (&forked_in).write_all(&[0])?;
            (&released_out).read_exact(&mut [0])
        });
    }
    let threads = os::threads().unwrap();
    let child = threads
        .start("child", move || async move {
            assert!(command.status().unwrap().success());
        })
        .unwrap();
    (&forked).read_exact(&mut [0]).unwrap();
    drop(file);
    let reopened = runtime.block_on(files.open(a, Mode::Write)).map(drop);
    (&released).write_all(&[0]).unwrap();
    child.join().unwrap();
    let opened = runtime.block_on(files.open(a, Mode::Write)).map(drop);
    assert_eq!(reopened, Ok(()));
    assert_eq!(opened, Ok(()));
    drop(files);
    thread.join().unwrap();
}

#[test]
fn a_child_does_not_hold_a_file_that_open_absolute_opened() {
    let scratch = Scratch::new();
    let (files, thread) = files(&scratch.0, "files");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let path = scratch.0.join("token");
    std::fs::write(&path, b"abc").unwrap();
    let absolute = Absolute::new(path.to_str().unwrap()).unwrap();
    let file = runtime.block_on(files.open_absolute(&absolute)).unwrap();
    // On macOS, a stat of `/dev/fd/N` gives the inode of the file, but the device of
    // `/dev/fd`, so only the inodes are compared.
    let inode = std::fs::metadata(&path).unwrap().ino();
    let fd = (0..1_024)
        .find(|fd| {
            let found = std::fs::metadata(format!("/dev/fd/{fd}"));
            found.is_ok_and(|found| found.ino() == inode)
        })
        .expect("the file is open");
    let listed = Command::new("ls")
        .arg("-iLd")
        .arg(format!("/dev/fd/{fd}"))
        .output()
        .unwrap();
    drop(file);
    let listed = String::from_utf8(listed.stdout).unwrap();
    let held = listed.split_whitespace().next() == Some(&inode.to_string());
    assert!(!held, "the child holds descriptor {fd}: {listed}");
    drop(files);
    thread.join().unwrap();
}
