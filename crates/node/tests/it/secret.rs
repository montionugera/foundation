//! The file store of secrets on the real disk reads a secret from the file that its
//! own text names, in the data directory that `foundation start --data` gets, unless
//! other users can read it, and `create_secret` writes each name there.

use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use env::files::{Absolute, Files};
use secret::{Error, Resolver, Source};
use types::name::Name;

#[test]
fn reads_the_secret_from_the_file_that_its_text_names() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-path-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let file = dir.join("data/secrets/site.token");
    let get = threads.start("get", move || async move {
        let resolver =
            Resolver::new(Arc::default(), Files::new(disk), Path::new("secrets"));
        let source = Source::Secret("site.token".parse().expect("a valid name"));
        let missing = resolver
            .get(&source)
            .await
            .map(|value| value.expose().to_vec());
        let Err(Error::Missing { detail }) = missing else {
            panic!("no value yet: {missing:?}");
        };
        assert_eq!(
            detail,
            format!(
                "the file {shown} is not there. Write the value to it, unreadable by \
                 other users: chmod o-r {shown}",
                shown = file.display()
            )
        );
        std::fs::create_dir_all(file.parent().expect("a directory"))
            .expect("makes the directory");
        std::fs::write(&file, b"t\n").expect("writes it");
        std::fs::set_permissions(&file, Permissions::from_mode(0o604))
            .expect("lets other users read it");
        let exposed = resolver
            .get(&source)
            .await
            .map(|value| value.expose().to_vec());
        let shown = file.display();
        let refused = Err(Error::Unavailable {
            detail: format!(
                "other users can read the file {shown}. Make it unreadable by other \
                 users: chmod o-r {shown}"
            ),
        });
        assert_eq!(exposed, refused);
        std::fs::set_permissions(&file, Permissions::from_mode(0o660))
            .expect("gives the group read and write, as `fsGroup` does");
        let read = resolver
            .get(&source)
            .await
            .map(|value| value.expose().to_vec());
        assert_eq!(read, Ok(b"t".to_vec()));
    });
    get.expect("the thread starts")
        .join()
        .expect("the get ends");
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
}

#[test]
fn reads_a_file_outside_the_data_directory_that_all_users_can_read() {
    let dir = std::env::temp_dir()
        .join(format!("foundation-secret-absolute-{}", std::process::id()));
    let threads = os::threads().expect("the OS gives the cores");
    let (disk, io) = os::files(&dir, &threads, "files").expect("the disk opens");
    let file = dir.join("token");
    std::fs::write(&file, b"t\r\n").expect("writes it");
    std::fs::set_permissions(&file, Permissions::from_mode(0o644))
        .expect("lets all users read it");
    let get = threads.start("get", move || async move {
        let resolver =
            Resolver::new(Arc::default(), Files::new(disk), Path::new("secrets"));
        let path = Absolute::new(file.to_str().expect("a UTF-8 path"))
            .expect("an absolute path");
        let read = resolver.get(&Source::File(path)).await;
        assert_eq!(read.map(|value| value.expose().to_vec()), Ok(b"t".to_vec()));
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
        let secrets = connector::testing::create_secrets(files);
        let read = secrets.get(&Source::Secret(name)).await;
        let read = read.map(|value| value.expose().to_vec());
        assert_eq!(read, Ok(b"t1".to_vec()));
    });
    let joined = put.expect("the thread starts").join();
    io.join().expect("the I/O thread ends");
    std::fs::remove_dir_all(&dir).expect("removes the directory");
    joined.expect("the put ends");
}
