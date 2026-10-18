//! The drop of `free` on a disk that no other process writes.

use std::path::Path;
use std::task::{Context, Waker};

use env::files::{Files, Mode};

const LEN: u64 = 8 << 20;

/// Creates the file `a` of 8 MiB through `files`, holds it until after the next `free`,
/// and asserts that the free is less by exactly its length. No other process may write
/// the disk.
pub(crate) async fn held(files: &Files) {
    let before = files.free().await.unwrap();
    let file = files.open(Path::new("a"), Mode::Create { len: LEN });
    let file = file.await.unwrap();
    let after = files.free().await.unwrap();
    file.close().await;
    assert_less_by_len(before, after);
}

/// Polls a create of the file `a` of 8 MiB through `files` once, while the guard that
/// `hold` gives keeps the disk from finishing it, and drops the create, then the guard.
/// Asserts that the next `free` is less by exactly its length. No other process may
/// write the disk.
pub(crate) async fn dropped<G>(files: &Files, hold: impl FnOnce() -> G) {
    let before = files.free().await.unwrap();
    let held = hold();
    let mut made = Box::pin(files.open(Path::new("a"), Mode::Create { len: LEN }));
    let mut context = Context::from_waker(Waker::noop());
    let pending = made.as_mut().poll(&mut context).is_pending();
    drop(made);
    drop(held);
    assert!(pending, "the create ended before its first poll");
    let after = files.free().await.unwrap();
    assert_less_by_len(before, after);
}

fn assert_less_by_len(before: u64, after: u64) {
    assert_eq!(
        before.checked_sub(after),
        Some(LEN),
        "{before} bytes free before, {after} after"
    );
}
