//! The drop of `free` on a disk that no other process writes.

use std::path::Path;

use env::files::{Files, Mode};

use crate::fifo::stalled;

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

/// Polls a create of the file `a` of 8 MiB through `files` once and drops it, then
/// polls a `free` once, while the I/O thread blocks, so both wait in its queue. `data`
/// is the data directory of `files`. Asserts that the free is less by exactly the
/// length of the create. No other process may write the disk.
pub(crate) async fn dropped(files: &Files, data: &Path) {
    let before = files.free().await.unwrap();
    let free = stalled(files, data, |context| {
        let mode = Mode::Create { len: LEN };
        let mut create = Box::pin(files.open(Path::new("a"), mode));
        let pending = create.as_mut().poll(context).is_pending();
        assert!(pending, "the create ended while the I/O thread was stalled");
        drop(create);
        let mut free = Box::pin(files.free());
        let pending = free.as_mut().poll(context).is_pending();
        assert!(pending, "the free ended while the I/O thread was stalled");
        free
    })
    .await;
    assert_less_by_len(before, free.await.unwrap());
}

fn assert_less_by_len(before: u64, after: u64) {
    assert_eq!(
        before.checked_sub(after),
        Some(LEN),
        "{before} bytes free before, {after} after"
    );
}
