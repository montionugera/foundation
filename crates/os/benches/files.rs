//! The cost of a call of an `os::files` handle on the real disk, and of a write open
//! and its close, which takes and releases the lock of the file.

use std::path::{Path, PathBuf};

use block::{Block, Pool};
use divan::Bencher;
use divan::counter::ItemsCount;
use env::files::{Files, Mode};

const CALLS: usize = 1_000;
const LEN: u64 = 1 << 20;
const PART: usize = 4_096;

fn main() {
    divan::main();
}

/// A file `f` of `LEN` bytes in a directory of its own, and its disk.
struct Setup {
    dir: PathBuf,
    files: Option<Files>,
    thread: Option<env::thread::Handle>,
    runtime: tokio::runtime::Runtime,
}

impl Setup {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("foundation-os-bench-{}-{tag}", std::process::id()));
        std::fs::create_dir(&dir).expect("the bench directory is new");
        let threads = os::threads().expect("the OS starts threads");
        let (disk, thread) =
            os::files(&dir, &threads, "bench").expect("the disk opens");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("the runtime builds");
        let files = Files::new(disk);
        runtime.block_on(async {
            let file = files.open(Path::new("f"), Mode::Create { len: LEN }).await;
            file.expect("the file is made").close().await;
        });
        Self {
            dir,
            files: Some(files),
            thread: Some(thread),
            runtime,
        }
    }

    fn files(&self) -> &Files {
        self.files
            .as_ref()
            .expect("the disk is open until the drop")
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        drop(self.files.take());
        if let Some(thread) = self.thread.take() {
            thread.join().expect("the I/O thread ends");
        }
        std::fs::remove_dir_all(&self.dir).expect("the bench directory is removed");
    }
}

/// One `write_at` of one 4 KiB part, awaited, on a file open to write.
#[divan::bench(sample_count = 100)]
fn write_at(bencher: Bencher<'_, '_>) {
    let setup = Setup::new("write");
    let config = block::Config::new(1 << 20).expect("the config is valid");
    let pool = Pool::new(config.clone(), block::Heap::new(config.reservation()));
    let mut unique = pool.alloc(PART).expect("the pool has room");
    unique.fill(7);
    let parts: [Block; 1] = [unique.freeze()];
    let file = setup
        .runtime
        .block_on(setup.files().open(Path::new("f"), Mode::Write))
        .expect("the file opens to write");
    let slots = LEN / PART as u64;
    bencher.counter(ItemsCount::new(CALLS)).bench_local(|| {
        setup.runtime.block_on(async {
            for i in 0..CALLS as u64 {
                let written = file.write_at((i % slots) * PART as u64, &parts).await;
                written.expect("the write ends");
            }
        });
    });
    setup.runtime.block_on(file.close());
}

fn open_close(bencher: Bencher<'_, '_>, mode: Mode, tag: &str) {
    let setup = Setup::new(tag);
    let files = setup.files();
    bencher.counter(ItemsCount::new(CALLS)).bench_local(|| {
        setup.runtime.block_on(async {
            for _ in 0..CALLS {
                let file = files.open(Path::new("f"), mode).await;
                file.expect("the file opens").close().await;
            }
        });
    });
}

/// One write open and its close, each awaited.
#[divan::bench(sample_count = 100)]
fn write_open_close(bencher: Bencher<'_, '_>) {
    open_close(bencher, Mode::Write, "wopen");
}

/// One read open and its close, each awaited: no lock, so no unlock.
#[divan::bench(sample_count = 100)]
fn read_open_close(bencher: Bencher<'_, '_>) {
    open_close(bencher, Mode::Read, "ropen");
}
