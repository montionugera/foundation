//! The drop of `free` on a disk that no other process writes.

use std::path::Path;
use std::task::{Context, Waker};

use env::files::{Files, Mode};

/// What the caller of a create does with its future.
#[derive(Clone, Copy)]
pub(crate) enum Create {
    /// Awaits it, and holds the file until after the next `free`.
    Held,
    /// Polls it once, and drops it.
    Dropped,
}

/// Creates the file `a` of 8 MiB through `files`, as `create` says, and asserts that
/// the next `free` is less by exactly its length. No other process may write the disk.
pub(crate) async fn check(files: &Files, create: Create) {
    const LEN: u64 = 8 << 20;
    let before = files.free().await.unwrap();
    let made = files.open(Path::new("a"), Mode::Create { len: LEN });
    let file = match create {
        Create::Held => Some(made.await.unwrap()),
        Create::Dropped => {
            let mut made = Box::pin(made);
            let mut context = Context::from_waker(Waker::noop());
            assert!(made.as_mut().poll(&mut context).is_pending());
            None
        }
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
