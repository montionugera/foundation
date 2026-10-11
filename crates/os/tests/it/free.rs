//! The drop of `free` on a disk that no other process writes.

use std::path::Path;
use std::task::{Context, Waker};

use env::files::{Files, Mode};

/// Creates the file `a` of 8 MiB through `files`, and asserts that the next `free` is
/// less by exactly its length. When `dropped`, the create is polled once and dropped.
/// No other process may write the disk.
pub(crate) async fn check(files: &Files, dropped: bool) {
    const LEN: u64 = 8 << 20;
    let before = files.free().await.unwrap();
    let create = files.open(Path::new("a"), Mode::Create { len: LEN });
    let file = if dropped {
        let mut create = Box::pin(create);
        let mut context = Context::from_waker(Waker::noop());
        assert!(create.as_mut().poll(&mut context).is_pending());
        None
    } else {
        Some(create.await.unwrap())
    };
    let after = files.free().await.unwrap();
    if let Some(file) = file {
        file.close().await;
    }
    assert_eq!(
        before.checked_sub(after),
        Some(LEN),
        "{before} bytes free before, {after} after"
    );
}
