//! `os::paths` on the real files of the host.

use std::fs::{self, Permissions};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use env::paths::{Error, Found};
use rustix::io::Errno;

use crate::cases::{self, Host};
use crate::disk::{Scratch, mode};
use crate::files::mkfifo;

fn run(case: fn(&Host<'_>)) {
    let scratch = Scratch::new();
    case(&Host {
        paths: &os::paths(),
        dir: &scratch.0,
        make_dir,
    });
}

fn make_dir(path: &Path) {
    fs::create_dir(path).unwrap();
}

/// A directory that takes no new file: a write that makes one in it gives `EACCES`.
fn sealed(scratch: &Scratch) -> PathBuf {
    let dir = scratch.0.join("sealed");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, Permissions::from_mode(0o500)).unwrap();
    dir
}

cases::tests!(run);

#[test]
fn a_write_with_no_directory_is_not_found_and_leaves_nothing() {
    let scratch = Scratch::new();
    let paths = os::paths();
    let path = scratch.0.join("none/plant.plan");
    assert_eq!(paths.write(&path, b"plan"), Err(Error::NotFound));
    assert_eq!(paths.read(&scratch.0, 0), Ok(Found::Directory(Vec::new())));
}

#[test]
fn a_path_with_a_nul_byte_is_an_invalid_argument() {
    let paths = os::paths();
    let invalid = Err(Error::Io { code: 22 });
    assert_eq!(paths.read(Path::new("a\0b"), 1), invalid);
    assert_eq!(
        paths.write(Path::new("a\0b"), b"x"),
        invalid.map(|_: Found| ())
    );
}

#[test]
fn a_new_file_gives_other_users_no_access() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.plan");
    os::paths().write(&path, b"plan").unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn a_write_of_a_name_of_the_longest_length_succeeds() {
    let scratch = Scratch::new();
    let path = scratch.0.join("a".repeat(255));
    let paths = os::paths();
    paths.write(&path, b"plan").unwrap();
    assert_eq!(paths.read(&path, 4), Ok(Found::File(b"plan".to_vec())));
}

#[test]
fn a_write_gives_the_group_and_other_users_no_access_whatever_the_old_mode() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    for old in [0o600, 0o664, 0o4755, 0o2755, 0o1755] {
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(old)).unwrap();
        assert_eq!(mode(&path), old);
        os::paths().write(&path, b"new").unwrap();
        assert_eq!(mode(&path), 0o600, "{old:o}");
    }
}

#[test]
fn a_write_on_a_path_that_ends_in_a_slash_keeps_the_files_inside() {
    let scratch = Scratch::new();
    let config = scratch.0.join("config");
    fs::create_dir(&config).unwrap();
    fs::write(config.join(".new"), b"user").unwrap();
    let slashed = PathBuf::from(format!("{}/", config.display()));
    let paths = os::paths();
    assert_eq!(paths.write(&slashed, b"y"), Err(Error::Io { code: 21 }));
    let user = Ok(Found::File(b"user".to_vec()));
    assert_eq!(paths.read(&config.join(".new"), 4), user);
    let entries = Ok(Found::Directory(vec![config]));
    assert_eq!(paths.read(&scratch.0, 1 << 12), entries);
}

#[test]
fn a_write_on_a_path_with_no_name_writes_nothing() {
    let scratch = Scratch::new();
    let config = scratch.0.join("config");
    fs::create_dir_all(config.join("c")).unwrap();
    let paths = os::paths();
    let on_dir = Err(Error::Io { code: 21 });
    for path in ["/", ".", ".."].map(PathBuf::from) {
        assert_eq!(paths.write(&path, b"x"), on_dir, "{}", path.display());
    }
    for path in [config.join(".."), config.join("./"), config.join("c/..")] {
        assert_eq!(paths.write(&path, b"x"), on_dir, "{}", path.display());
    }
    let entries = Ok(Found::Directory(vec![config.join("c")]));
    assert_eq!(paths.read(&config, 1 << 12), entries);
}

#[test]
fn a_write_on_a_path_that_names_a_directory_makes_no_file_beside_it() {
    let scratch = Scratch::new();
    let dir = sealed(&scratch);
    for none in [dir.join("none/"), dir.join("none/.")] {
        let written = os::paths().write(&none, b"x");
        assert_eq!(written, Err(Error::NotFound), "{}", none.display());
    }
}

/// Another thread makes and removes `config`, so a stat that finds nothing does not
/// show that the open of a hidden file under `config/..` fails.
#[test]
fn a_write_on_a_path_that_ends_in_two_dots_makes_no_file_beside_the_directory() {
    let scratch = Scratch::new();
    let config = scratch.0.join("config");
    let dots = config.join("..");
    let stopped = AtomicBool::new(false);
    let named = [
        Err(Error::NotFound),
        Err(Error::Io {
            code: Errno::ISDIR.raw_os_error(),
        }),
    ];
    let other = thread::scope(|scope| {
        #[expect(clippy::disallowed_methods, reason = "the test races the write")]
        scope.spawn(|| {
            while !stopped.load(Ordering::Relaxed) {
                fs::create_dir(&config).unwrap();
                fs::remove_dir(&config).unwrap();
            }
        });
        let paths = os::paths();
        let mut written = (0..20_000).map(|_| paths.write(&dots, b"x"));
        let other = written.find(|written| !named.contains(written));
        stopped.store(true, Ordering::Relaxed);
        other
    });
    let left = fs::read_dir(&scratch.0).unwrap().count();
    assert_eq!((other, left), (None, 0));
}

#[test]
fn a_write_of_a_name_that_is_too_long_makes_no_file_beside_it() {
    let scratch = Scratch::new();
    let long = sealed(&scratch).join("a".repeat(256));
    let code = Errno::NAMETOOLONG.raw_os_error();
    assert_eq!(os::paths().write(&long, b"x"), Err(Error::Io { code }));
}

#[test]
fn a_write_leaves_the_old_bytes_at_another_hard_link_of_the_file() {
    let scratch = Scratch::new();
    let (path, twin) = (scratch.0.join("plant.hcl"), scratch.0.join("twin.hcl"));
    fs::write(&path, b"old").unwrap();
    fs::hard_link(&path, &twin).unwrap();
    let paths = os::paths();
    paths.write(&path, b"new").unwrap();
    let read = [&path, &twin].map(|path| paths.read(path, 3));
    let [new, old] = [b"new", b"old"].map(|bytes| Ok(Found::File(bytes.to_vec())));
    assert_eq!(read, [new, old]);
}

#[test]
fn a_write_leaves_the_old_file_whole_for_a_reader_that_has_it_open() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    #[expect(clippy::disallowed_methods, reason = "the test holds the old file")]
    let mut old = fs::File::open(&path).unwrap();
    os::paths().write(&path, b"new").unwrap();
    let mut bytes = Vec::new();
    old.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"old");
}

#[test]
fn a_write_on_a_device_writes_in_place() {
    assert_eq!(os::paths().write(Path::new("/dev/null"), b"plan"), Ok(()));
    let null = fs::symlink_metadata("/dev/null").unwrap();
    assert!(null.file_type().is_char_device(), "{null:?}");
}

#[test]
fn a_write_on_a_pipe_gives_the_bytes_to_its_reader() {
    let scratch = Scratch::new();
    let pipe = scratch.0.join("pipe");
    mkfifo(&pipe);
    let nonblocking = rustix::fs::OFlags::NONBLOCK.bits().cast_signed();
    let mut reader = fs::OpenOptions::new();
    #[expect(clippy::disallowed_methods, reason = "the test reads the pipe")]
    let mut reader = reader
        .read(true)
        .custom_flags(nonblocking)
        .open(&pipe)
        .unwrap();
    let paths = os::paths();
    assert_eq!(paths.write(&pipe, b"plan"), Ok(()));
    let mut bytes = [0; 8];
    let read = reader.read(&mut bytes).unwrap();
    assert_eq!(&bytes[..read], b"plan");
    assert!(fs::symlink_metadata(&pipe).unwrap().file_type().is_fifo());
    assert_eq!(
        paths.read(&scratch.0, 1 << 12),
        Ok(Found::Directory(vec![pipe]))
    );
}

#[test]
fn a_write_on_a_link_writes_the_file_it_names() {
    let scratch = Scratch::new();
    let (file, link) = (scratch.0.join("plant.hcl"), scratch.0.join("link"));
    fs::write(&file, b"old file").unwrap();
    symlink(&file, &link).unwrap();
    let paths = os::paths();
    paths.write(&link, b"new").unwrap();
    assert_eq!(paths.read(&file, 3), Ok(Found::File(b"new".to_vec())));
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    let entries = Ok(Found::Directory(vec![link, file]));
    assert_eq!(paths.read(&scratch.0, 1 << 12), entries);
}

#[test]
fn a_write_on_a_link_to_nothing_is_not_found_and_makes_no_file() {
    let scratch = Scratch::new();
    let (target, link) = (scratch.0.join("plant.hcl"), scratch.0.join("link"));
    symlink(&target, &link).unwrap();
    let paths = os::paths();
    assert_eq!(paths.write(&link, b"new"), Err(Error::NotFound));
    assert!(fs::symlink_metadata(&link).unwrap().is_symlink());
    assert_eq!(paths.read(&target, 1), Err(Error::NotFound));
    let entries = Ok(Found::Directory(vec![link]));
    assert_eq!(paths.read(&scratch.0, 1 << 12), entries);
}

#[test]
fn two_writers_of_one_path_each_write_whole() {
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.plan");
    let (a, b) = (vec![b'a'; 1 << 20], vec![b'b'; 1 << 20]);
    for _ in 0..8 {
        #[expect(clippy::disallowed_methods, reason = "os is the crate under test")]
        std::thread::scope(|scope| {
            let writers =
                [&a, &b].map(|bytes| scope.spawn(|| os::paths().write(&path, bytes)));
            for writer in writers {
                assert_eq!(writer.join().unwrap(), Ok(()));
            }
        });
        let read = os::paths().read(&path, 1 << 20);
        let whole = [&a, &b].map(|bytes| Ok(Found::File(bytes.clone())));
        assert!(whole.contains(&read), "a mixed or failed write");
    }
    let entries = Ok(Found::Directory(vec![path]));
    assert_eq!(os::paths().read(&scratch.0, 1 << 12), entries);
}
