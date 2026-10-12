//! A child holds a copy of each descriptor of this process from its fork. It holds
//! each one that is closed on exec, as each file of `os::files` is, until its exec, or
//! to its end.

// Lets Clippy treat the helpers as test code.
#![cfg(test)]

#[path = "common/mod.rs"]
#[expect(dead_code, reason = "this binary uses some of the helpers")]
mod disk;

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use disk::{Scratch, files};
use env::files::Mode;

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
