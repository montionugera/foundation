//! A get of the secret store of `node`, on the real disk, leaves the value in no
//! freed block: a get that reads it, a get whose read fails after it filled part of
//! the block, and a get that drops while its read waits. The scan covers every thread,
//! so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::future::poll_fn;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::task::Poll;

use env::files::{Files, Mode};
use secret::Reads;
use secret::store::{Error, File, Request, Store};
use types::name::Name;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const PLAIN: [u8; 32] = [0x5a; 32];

fn main() {
    let found = [
        ("reads", freed("reads", reads)),
        ("cut", freed("cut", cut)),
        ("dropped", freed("dropped", dropped)),
    ];
    assert_eq!(
        found,
        [("reads", 0), ("cut", 0), ("dropped", 0)],
        "a get of the file store leaves the value in no freed block"
    );
}

/// Runs `case` on a shard with the real disk of a new directory, whose file
/// `data/secrets/site.token` holds `PLAIN` and a line end. Gives the count of freed
/// blocks that hold `PLAIN`, from the start of `case` to the end of the I/O thread.
fn freed<F>(name: &str, case: fn(Files, PathBuf) -> F) -> u64
where
    F: Future<Output = ()> + 'static,
{
    let dir = std::env::temp_dir()
        .join(format!("foundation-erase-{}-{name}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let data = dir.join("data");
    std::fs::create_dir(data.join("secrets")).expect("makes the directory");
    let token = [&PLAIN[..], b"\n"].concat();
    std::fs::write(data.join("secrets/site.token"), token).expect("writes it");
    let ((), found) = ALLOCATOR.freed_holding(&PLAIN, || {
        let get = threads.start("get", move || case(Files::new(disk), data));
        get.expect("the thread starts")
            .join()
            .expect("the get ends");
        io.join().expect("the I/O thread ends");
    });
    std::fs::remove_dir_all(&dir).expect("removes the directory");
    found
}

async fn reads(files: Files, _: PathBuf) {
    let store = File::new(Reads::new(files), Path::new("secrets"));
    let value = store.get(&name()).await.expect("the file holds the value");
    assert_eq!(value.expose(), PLAIN, "the file store reads the value");
}

/// Cuts the file to `PLAIN` after the get opened it, so the read fills 32 of its 33
/// bytes and fails.
async fn cut(files: Files, data: PathBuf) {
    let store = File::new(Reads::new(files.clone()), Path::new("secrets"));
    let name = name();
    let mut get = store.get(&name);
    hold_read(&files, &data, &mut get).await;
    std::fs::write(data.join("secrets/site.token"), PLAIN).expect("cuts the file");
    release(&data, "b");
    let failed = Err(Error::Unavailable {
        detail: "read_at of secrets/site.token failed with OS error 5".into(),
    });
    let got = get.await.map(|value| value.expose().to_vec());
    assert_eq!(got, failed, "a read that ends short fails");
}

/// Drops the get while its read waits on the I/O thread.
async fn dropped(files: Files, data: PathBuf) {
    let store = File::new(Reads::new(files.clone()), Path::new("secrets"));
    let name = name();
    let mut get = store.get(&name);
    hold_read(&files, &data, &mut get).await;
    drop(get);
    release(&data, "b");
}

/// Polls `get` until its read waits on the I/O thread behind an open of the FIFO
/// `b`, which [`release`] ends. An open of the FIFO `a` holds the I/O thread at the
/// first poll, so the open of the file ends only after it, and a list after that open
/// tells when it ended.
async fn hold_read(files: &Files, data: &Path, get: &mut Request<'_>) {
    for fifo in ["a", "b"] {
        let made = Command::new("mkfifo").arg(data.join(fifo)).status();
        assert!(
            made.expect("mkfifo runs").success(),
            "makes the FIFO {fifo}"
        );
    }
    let mut a = Box::pin(files.open(Path::new("a"), Mode::Read));
    assert!(
        once(a.as_mut()).await.is_pending(),
        "`a` waits for a writer"
    );
    assert!(
        once(get.as_mut()).await.is_pending(),
        "the open waits behind `a`"
    );
    let mut listed = Box::pin(files.list(Path::new("secrets")));
    assert!(
        once(listed.as_mut()).await.is_pending(),
        "the list waits behind `a`"
    );
    let mut b = Box::pin(files.open(Path::new("b"), Mode::Read));
    assert!(once(b.as_mut()).await.is_pending(), "`b` waits behind `a`");
    // The I/O thread runs a call that it took also when its future drops.
    drop(b);
    release(data, "a");
    drop(a.await.expect("opens `a`"));
    listed.await.expect("lists the directory");
    assert!(
        once(get.as_mut()).await.is_pending(),
        "the read waits behind `b`"
    );
}

/// Polls `future` once.
async fn once<F: Future + ?Sized>(mut future: Pin<&mut F>) -> Poll<F::Output> {
    poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await
}

/// Opens the FIFO `fifo` of `data` to write, which ends the open that waits on it.
#[expect(
    clippy::disallowed_methods,
    reason = "the other end of a FIFO that the disk under test waits on"
)]
fn release(data: &Path, fifo: &str) {
    let writer = std::fs::OpenOptions::new()
        .write(true)
        .open(data.join(fifo));
    drop(writer.expect("opens the FIFO"));
}

fn name() -> Name {
    "site.token".parse().expect("a valid name")
}
