//! `os::paths` on the real files of the host.

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use env::paths::{Error, Found};

use crate::cases::{self, Host};
use crate::disk::Scratch;

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

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
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
    fs::write(scratch.0.join("plant.plan.new"), b"old").unwrap();
    fs::set_permissions(
        scratch.0.join("plant.plan.new"),
        Permissions::from_mode(0o644),
    )
    .unwrap();
    os::paths().write(&path, b"plan").unwrap();
    assert_eq!(mode(&path), 0o600);
}

#[test]
fn a_write_keeps_the_mode_of_the_file_it_replaces() {
    let scratch = Scratch::new();
    let (private, open) = (scratch.0.join("secret.hcl"), scratch.0.join("plant.hcl"));
    for (path, mode) in [(&private, 0o600), (&open, 0o664)] {
        fs::write(path, b"old").unwrap();
        fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
        os::paths().write(path, b"new").unwrap();
    }
    assert_eq!((mode(&private), mode(&open)), (0o600, 0o664));
}

#[test]
fn a_write_on_a_path_that_ends_in_a_slash_keeps_the_files_inside() {
    let scratch = Scratch::new();
    let config = scratch.0.join("config");
    fs::create_dir(&config).unwrap();
    fs::write(config.join(".new"), b"user").unwrap();
    let slashed = PathBuf::from(format!("{}/", config.display()));
    let paths = os::paths();
    assert_eq!(paths.write(&slashed, b"y"), Err(Error::Io { code: 20 }));
    let user = Ok(Found::File(b"user".to_vec()));
    assert_eq!(paths.read(&config.join(".new"), 4), user);
    let entries = Ok(Found::Directory(vec![config]));
    assert_eq!(paths.read(&scratch.0, 0), entries);
}

#[test]
fn a_write_on_a_path_with_no_name_writes_nothing() {
    let paths = os::paths();
    assert_eq!(paths.write(Path::new(""), b"x"), Err(Error::NotFound));
    let on_dir = Err(Error::Io { code: 21 });
    for path in ["/", ".", "..", "a/.."] {
        assert_eq!(paths.write(Path::new(path), b"x"), on_dir, "{path}");
    }
}
