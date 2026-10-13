//! A get of the secret store of `node`, on the real disk, leaves the value in no
//! freed block. The scan covers every thread, so this binary has no test harness.

#![expect(clippy::disallowed_macros, reason = "COUNTING ALLOCATOR")]

use std::path::Path;

use env::files::Files;
use secret::store::{File, Store};
use types::name::Name;

#[global_allocator]
static ALLOCATOR: counting::Allocator = counting::Allocator::new();

const PLAIN: [u8; 32] = [0x5a; 32];

fn main() {
    let dir =
        std::env::temp_dir().join(format!("foundation-erase-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    std::fs::create_dir(dir.join("data/secrets")).expect("makes the directory");
    let token = [&PLAIN[..], b"\n"].concat();
    std::fs::write(dir.join("data/secrets/site.token"), token).expect("writes it");

    let ((), found) = ALLOCATOR.freed_holding(&PLAIN, || {
        let get = threads.start("get", move || async move {
            let store = File::new(Files::new(disk), Path::new("secrets"));
            let name: Name = "site.token".parse().expect("a valid name");
            let value = store.get(&name).await.expect("the file holds the value");
            assert_eq!(value.expose(), PLAIN, "the file store reads the value");
        });
        get.expect("the thread starts")
            .join()
            .expect("the get ends");
    });
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
    assert_eq!(
        found, 0,
        "a get of the file store leaves the value in no freed block"
    );
}
