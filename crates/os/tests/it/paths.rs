//! `os::paths` on the real files of the host.

use std::fs::{self, Permissions};
use std::io::Read;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt, chown, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

use env::paths::{Error, Found};
use rustix::fs::{CWD, RenameFlags, renameat_with};
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

/// Writes over a file of each old mode in `dir`, a directory of this user.
fn write_over_each_mode(dir: &Path) {
    // On macOS a file takes the group of its directory, and the OS drops the setgid
    // bit of a file whose group this user is not in.
    let group = rustix::process::getegid().as_raw();
    chown(dir, None, Some(group)).unwrap();
    let path = dir.join("plant.hcl");
    for old in [0o600, 0o664, 0o4755, 0o2755, 0o1755] {
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(old)).unwrap();
        assert_eq!(mode(&path), old);
        os::paths().write(&path, b"new").unwrap();
        assert_eq!(mode(&path), 0o600, "{old:o}");
    }
}

#[test]
fn a_write_gives_the_group_and_other_users_no_access_whatever_the_old_mode() {
    write_over_each_mode(&Scratch::new().0);
}

/// A directory in `/private/tmp` takes its group, `wheel`, which a user is not in.
#[cfg(target_os = "macos")]
#[test]
fn a_write_gives_no_access_in_a_directory_that_took_the_group_of_another() {
    let scratch = Scratch::under(Path::new("/private/tmp"));
    write_over_each_mode(&scratch.0);
}

/// `os` sets no access control list.
#[cfg(target_os = "macos")]
#[test]
fn a_write_takes_the_access_control_entries_that_its_directory_gives_new_files() {
    use std::process::Command;
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
    let entries = || {
        let listed = Command::new("/bin/ls")
            .arg("-le")
            .arg(&path)
            .output()
            .unwrap();
        assert!(listed.status.success(), "{listed:?}");
        let listed = String::from_utf8(listed.stdout).unwrap();
        listed
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(entries(), [""; 0]);
    let mut chmod = Command::new("/bin/chmod");
    chmod.args(["+a", "everyone allow read,file_inherit"]);
    assert!(chmod.arg(&scratch.0).status().unwrap().success());
    os::paths().write(&path, b"new").unwrap();
    assert_eq!(entries(), [" 0: group:everyone inherited allow read"]);
    assert_eq!(mode(&path), 0o600);
}

/// The `posix_acl_xattr` form of `entries`: a version, then a tag, the bits, and a user
/// for each.
#[cfg(target_os = "linux")]
fn access_list(entries: &[(u16, u16, u32)]) -> Vec<u8> {
    let mut bytes = 2u32.to_le_bytes().to_vec();
    for (tag, bits, user) in entries {
        bytes.extend(tag.to_le_bytes());
        bytes.extend(bits.to_le_bytes());
        bytes.extend(user.to_le_bytes());
    }
    bytes
}

/// `os` sets no access control list.
#[cfg(target_os = "linux")]
#[test]
fn a_write_takes_the_access_control_entries_that_its_directory_gives_new_files() {
    use rustix::fs::{XattrFlags, getxattr, setxattr};
    let (owner, named, group, mask, other) = (0x01, 0x02, 0x04, 0x10, 0x20);
    let (none, read, read_write, all, unnamed) = (0, 4, 6, 7, u32::MAX);
    let scratch = Scratch::new();
    let path = scratch.0.join("plant.hcl");
    fs::write(&path, b"old").unwrap();
    let access = "system.posix_acl_access";
    let old = access_list(&[
        (owner, read_write, unnamed),
        (named, read, 4343),
        (group, none, unnamed),
        (mask, read, unnamed),
        (other, none, unnamed),
    ]);
    setxattr(&path, access, &old, XattrFlags::empty()).unwrap();
    // A list of only the mode bits is stored as no list.
    let bare = scratch.0.join("bare.hcl");
    fs::write(&bare, b"old").unwrap();
    let mode_only = access_list(&[
        (owner, read_write, unnamed),
        (group, none, unnamed),
        (other, none, unnamed),
    ]);
    setxattr(&bare, access, &mode_only, XattrFlags::empty()).unwrap();
    let mut taken = [0; 64];
    assert_eq!(getxattr(&bare, access, &mut taken[..]), Err(Errno::NODATA));
    let given = access_list(&[
        (owner, all, unnamed),
        (named, read_write, 4242),
        (group, read, unnamed),
        (mask, all, unnamed),
        (other, none, unnamed),
    ]);
    let default = "system.posix_acl_default";
    setxattr(&scratch.0, default, &given, XattrFlags::empty()).unwrap();
    let expected = access_list(&[
        (owner, read_write, unnamed),
        (named, read_write, 4242),
        (group, read, unnamed),
        (mask, none, unnamed),
        (other, none, unnamed),
    ]);
    for path in [path, bare, scratch.0.join("site.hcl")] {
        os::paths().write(&path, b"new").unwrap();
        let len = getxattr(&path, access, &mut taken[..]).unwrap();
        assert_eq!(taken[..len], expected);
        assert_eq!(mode(&path), 0o600);
    }
    let tool = scratch.0.join("tool.hcl");
    let mut open = fs::OpenOptions::new();
    open.write(true).create_new(true).mode(0o666);
    #[expect(clippy::disallowed_methods, reason = "the test makes a file as a tool")]
    open.open(&tool).unwrap();
    let len = getxattr(&tool, access, &mut taken[..]).unwrap();
    let expected = access_list(&[
        (owner, read_write, unnamed),
        (named, read_write, 4242),
        (group, read, unnamed),
        (mask, read_write, unnamed),
        (other, none, unnamed),
    ]);
    assert_eq!(taken[..len], expected);
    assert_eq!(mode(&tool), 0o660);
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

/// The name of the hidden file has 21 bytes, so its path passes the longest path of
/// the OS where a shorter name is at that path.
#[test]
fn a_write_whose_hidden_file_has_a_path_that_is_too_long_makes_no_file() {
    let most = if cfg!(target_os = "macos") {
        1023
    } else {
        4095
    };
    let scratch = Scratch::new();
    // The OS counts the path behind each symbolic link.
    let mut dir = fs::canonicalize(&scratch.0).unwrap();
    while dir.as_os_str().len() + 224 < most {
        dir.push("d".repeat(200));
        fs::create_dir(&dir).unwrap();
    }
    let room = most - dir.as_os_str().len() - 1;
    let [short, same] = [20, 21].map(|name| {
        let dir = dir.join("e".repeat(room - name - 1));
        fs::create_dir(&dir).unwrap();
        let file = dir.join("f".repeat(name));
        assert_eq!(file.as_os_str().len(), most);
        file
    });
    let paths = os::paths();
    let code = Errno::NAMETOOLONG.raw_os_error();
    assert_eq!(paths.write(&short, b"new"), Err(Error::Io { code }));
    assert_eq!(paths.read(&short, 3), Err(Error::NotFound));
    fs::write(&short, b"old").unwrap();
    assert_eq!(paths.write(&short, b"new"), Err(Error::Io { code }));
    assert_eq!(paths.read(&short, 3), Ok(Found::File(b"old".to_vec())));
    let dir = short.parent().unwrap();
    assert_eq!(paths.read(dir, most), Ok(Found::Directory(vec![short])));
    assert_eq!(paths.write(&same, b"new"), Ok(()));
    assert_eq!(paths.read(&same, 3), Ok(Found::File(b"new".to_vec())));
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
fn a_write_on_a_link_leaves_the_mode_of_the_file_it_names() {
    let scratch = Scratch::new();
    let (file, link) = (scratch.0.join("plant.hcl"), scratch.0.join("link"));
    fs::write(&file, b"old").unwrap();
    fs::set_permissions(&file, Permissions::from_mode(0o644)).unwrap();
    symlink(&file, &link).unwrap();
    os::paths().write(&link, b"new").unwrap();
    assert_eq!(mode(&file), 0o644);
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

/// `link` names a directory or a file at each instant: another thread swaps it with one
/// rename. A read gives one of the two, never an error.
#[test]
fn a_read_of_a_path_that_swaps_a_directory_for_a_file_gives_one_of_them() {
    let scratch = Scratch::new();
    let (dir, file) = (scratch.0.join("dir"), scratch.0.join("file"));
    fs::create_dir(&dir).unwrap();
    fs::write(&file, b"x").unwrap();
    let (link, next) = (scratch.0.join("link"), scratch.0.join("next"));
    symlink(&dir, &link).unwrap();
    let swap = || {
        for target in [&file, &dir] {
            symlink(target, &next).unwrap();
            fs::rename(&next, &link).unwrap();
        }
    };
    let other = race(swap, 5_000, &link, |read| {
        matches!(read, Ok(Found::File(_) | Found::Directory(_)))
    });
    assert_eq!(other, None);
}

/// `plant` and `file` swap a directory and a file in one step.
#[test]
fn a_read_of_a_directory_that_becomes_a_file_gives_no_error() {
    let scratch = Scratch::new();
    let (plant, file) = (scratch.0.join("plant"), scratch.0.join("file"));
    fs::create_dir(&plant).unwrap();
    fs::write(&file, b"x").unwrap();
    let swap = || {
        renameat_with(CWD, &plant, CWD, &file, RenameFlags::EXCHANGE).unwrap();
    };
    let other = race(swap, 100_000, &plant, |read| {
        matches!(read, Ok(Found::File(_) | Found::Directory(_)))
    });
    assert_eq!(other, None);
}

/// A path through `/dev/fd` names a directory that is removed before the read opens it.
#[test]
fn a_read_of_a_removed_directory_lists_no_entries() {
    use std::os::fd::AsRawFd;
    let scratch = Scratch::new();
    let plant = scratch.0.join("plant");
    fs::create_dir(&plant).unwrap();
    #[expect(clippy::disallowed_methods, reason = "the test holds the directory")]
    let held = fs::File::open(&plant).unwrap();
    fs::remove_dir(&plant).unwrap();
    let path = PathBuf::from(format!("/dev/fd/{}", held.as_raw_fd()));
    assert_eq!(
        os::paths().read(&path, 1 << 12),
        Ok(Found::Directory(Vec::new()))
    );
}

/// Reads `path` until another thread has called `swap` `cycles` times or ended, and
/// gives the first read that `expected` refuses. The bound is on the swaps, not the
/// reads, so a slow or busy host still races each read against many swaps. On macOS
/// it stops at 1 000 swaps: the race shows there in the first swaps, and each swap is
/// slow.
fn race(
    swap: impl Fn() + Sync,
    cycles: usize,
    path: &Path,
    expected: impl Fn(&Result<Found, Error>) -> bool,
) -> Option<Result<Found, Error>> {
    let cycles = if cfg!(target_os = "macos") {
        cycles.min(1_000)
    } else {
        cycles
    };
    let (done, stopped) = (AtomicUsize::new(0), AtomicBool::new(false));
    thread::scope(|scope| {
        #[expect(clippy::disallowed_methods, reason = "the test races the read")]
        let swapper = scope.spawn(|| {
            while !stopped.load(Ordering::Relaxed) {
                swap();
                done.fetch_add(1, Ordering::Relaxed);
            }
        });
        let paths = os::paths();
        let mut other = None;
        // A panic in `swap` ends the swapper, and the scope then fails the test.
        while other.is_none()
            && done.load(Ordering::Relaxed) < cycles
            && !swapper.is_finished()
        {
            other = Some(paths.read(path, 1 << 12)).filter(|read| !expected(read));
        }
        stopped.store(true, Ordering::Relaxed);
        other
    })
}
