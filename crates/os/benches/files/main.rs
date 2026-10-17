//! The cost of a call of an `os::files` handle on the real disk, of a write open and
//! its close, which take and release the lock of the file, and of a read open and its
//! close, which take no lock.

#[cfg(any(target_os = "linux", target_os = "macos"))] // #2956
mod unix;

fn main() {
    divan::main();
}
