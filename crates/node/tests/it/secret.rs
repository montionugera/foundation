//! The file store of secrets on the real disk reads a secret from the file that its
//! own text names, in the data directory that `foundation start --data` gets.

use std::path::Path;

use env::files::Files;
use secret::Reads;
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
