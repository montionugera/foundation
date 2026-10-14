//! The file store of secrets on the real disk reads a secret from the file that its
//! own text names, in the data directory that `foundation start --data` gets, and
//! `create_secret` writes each name there.

use std::path::Path;

use env::files::Files;
use secret::store::{Error, File, Store};
use types::name::Name;

#[test]
fn reads_the_secret_from_the_file_that_its_text_names() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-path-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let file = dir.join("data/secrets/site.token");
    let get = threads.start("get", move || async move {
        let store = File::new(Files::new(disk), Path::new("secrets"));
        let name: Name = "site.token".parse().expect("a valid name");
        let missing = store.get(&name).await.map(|value| value.expose().to_vec());
        let Err(Error::Missing { detail }) = missing else {
            panic!("no value yet: {missing:?}");
        };
        assert_eq!(detail, format!("write it to the file {}", file.display()));
        std::fs::create_dir_all(file.parent().expect("a directory"))
            .expect("makes the directory");
        std::fs::write(&file, b"t\n").expect("writes it");
        let read = store.get(&name).await.map(|value| value.expose().to_vec());
        assert_eq!(read, Ok(b"t".to_vec()));
    });
    get.expect("the thread starts")
        .join()
        .expect("the get ends");
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
}

#[test]
fn create_secret_writes_a_name_of_the_most_bytes() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-longest-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let put = threads.start("put", move || async move {
        let files = Files::new(disk);
        let text = format!("a.{}", "b".repeat(Name::MAX_BYTES - 2));
        let name: Name = text.parse().expect("a valid name");
        connector::testing::create_secret(&files, &name, b"t1").await;
        let store = connector::testing::create_secret_store(files);
        let read = store.get(&name).await.map(|value| value.expose().to_vec());
        assert_eq!(read, Ok(b"t1".to_vec()));
    });
    let joined = put.expect("the thread starts").join();
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
    joined.expect("the put ends");
}
