//! The file store of secrets on the real disk reads a secret from the file that its
//! own text names, in the data directory that `foundation start --data` gets.

use std::path::Path;
use std::sync::Arc;

use env::files::Files;
use secret::store::{Error, File, Store};
use secret::{Reads, Resolver, Source};
use types::name::Name;

#[test]
fn reads_the_secret_from_the_file_that_its_text_names() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-path-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let file = dir.join("data/secrets/site.token");
    let get = threads.start("get", move || async move {
        let store = File::new(Reads::new(Files::new(disk)), Path::new("secrets"));
        let name: Name = "site.token".parse().expect("a valid name");
        let missing = store.get(&name).await.map(|value| value.expose().to_vec());
        let Err(Error::Missing { detail }) = missing else {
            panic!("no value yet: {missing:?}");
        };
        assert_eq!(
            detail,
            format!(
                "the file {} is not there. Write the value to it",
                file.display()
            )
        );
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
fn reads_a_file_at_an_absolute_path_outside_the_data_directory() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-absolute-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let file = dir.join("token");
    std::fs::write(&file, b"t\r\n").expect("writes it");
    let get = threads.start("get", move || async move {
        let reads = Reads::new(Files::new(disk));
        let store = File::new(reads.clone(), Path::new("secrets"));
        let resolver = Resolver::new(Arc::default(), reads, Box::new(store));
        let read = resolver.get(&Source::File(file)).await;
        assert_eq!(read.map(|value| value.expose().to_vec()), Ok(b"t".to_vec()));
    });
    get.expect("the thread starts")
        .join()
        .expect("the get ends");
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
}
