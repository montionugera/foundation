//! The drop of `free` on a disk that no other process writes.

use std::path::Path;
use std::task::{Context, Waker};

use env::files::{Files, Mode};

/// Creates the file `a` of 8 MiB through `files`, and asserts that `free` drops by
/// exactly its length. No other process may write the disk.
pub(crate) async fn check(files: &Files) {
    const LEN: u64 = 8 << 20;
    let before = files.free().await.unwrap();
    let mode = Mode::Create { len: LEN };
    let file = files.open(Path::new("a"), mode).await.unwrap();
    let after = files.free().await.unwrap();
    file.close().await;
    assert_eq!(
        before.checked_sub(after),
        Some(LEN),
        "{before} bytes free before, {after} after"
    );
}

/// Polls a create of the file `a` of 8 MiB through `files` once and drops it, and
/// asserts that the next `free` is less by exactly its length. No other process may
/// write the disk.
pub(crate) async fn check_dropped(files: &Files) {
    const LEN: u64 = 8 << 20;
    let before = files.free().await.unwrap();
    let mut context = Context::from_waker(Waker::noop());
    let mut made = Box::pin(files.open(Path::new("a"), Mode::Create { len: LEN }));
    assert!(made.as_mut().poll(&mut context).is_pending());
    drop(made);
    let after = files.free().await.unwrap();
    assert_eq!(
        before.checked_sub(after),
        Some(LEN),
        "{before} bytes free before, {after} after"
    );
}
