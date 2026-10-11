//! The cases of `env::paths` that `os` and `sim` both pass.

use std::path::Path;

use env::paths::{Error, Found, Paths};

/// What a case runs on.
pub(crate) struct Host<'a> {
    pub(crate) paths: &'a Paths,
    /// A directory that is there and empty. On `sim`, only `/` is there with no file
    /// under it.
    pub(crate) dir: &'a Path,
    /// Makes a directory in a directory that is there, on a host where a write does
    /// not.
    pub(crate) make_dir: fn(&Path),
}

/// `ENOTDIR` and `EISDIR` on Linux and macOS.
const NOT_DIRECTORY: i32 = 20;
const DIRECTORY: i32 = 21;

pub(crate) fn a_file_at_the_bound_reads_whole(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let path = dir.join("plant.hcl");
    paths.write(&path, b"node {}").unwrap();
    assert_eq!(paths.read(&path, 7), Ok(Found::File(b"node {}".to_vec())));
    let empty = dir.join("empty");
    paths.write(&empty, b"").unwrap();
    assert_eq!(paths.read(&empty, 0), Ok(Found::File(Vec::new())));
}

pub(crate) fn a_file_over_the_bound_reads_large(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let path = dir.join("plant.hcl");
    paths.write(&path, b"node {}").unwrap();
    assert_eq!(paths.read(&path, 6), Ok(Found::Large));
    assert_eq!(paths.read(&path, 0), Ok(Found::Large));
}

pub(crate) fn a_directory_lists_in_name_order(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let config = dir.join("config");
    assert_eq!(paths.read(dir, 0), Ok(Found::Directory(Vec::new())));
    (host.make_dir)(&config);
    (host.make_dir)(&config.join("c"));
    for name in ["b.hcl", "a.hcl", "c/d.hcl", "a-b.hcl"] {
        paths.write(&config.join(name), b"x").unwrap();
    }
    let entries = ["a-b.hcl", "a.hcl", "b.hcl", "c"].map(|name| config.join(name));
    assert_eq!(
        paths.read(&config, 0),
        Ok(Found::Directory(entries.to_vec()))
    );
    assert_eq!(paths.read(dir, 0), Ok(Found::Directory(vec![config])));
}

pub(crate) fn a_path_with_nothing_is_not_found(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    assert_eq!(paths.read(&dir.join("none"), 1), Err(Error::NotFound));
    assert_eq!(paths.read(&dir.join("none/a"), 1), Err(Error::NotFound));
}

pub(crate) fn a_write_replaces_the_file(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let path = dir.join("plant.plan");
    paths.write(&path, b"first plan").unwrap();
    paths.write(&path, b"second").unwrap();
    assert_eq!(paths.read(&path, 64), Ok(Found::File(b"second".to_vec())));
    assert_eq!(paths.read(dir, 0), Ok(Found::Directory(vec![path])));
}

pub(crate) fn a_path_through_a_file_fails(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let file = dir.join("plant.hcl");
    paths.write(&file, b"x").unwrap();
    let under = file.join("a");
    let through = Error::Io {
        code: NOT_DIRECTORY,
    };
    assert_eq!(paths.read(&under, 1), Err(through));
    assert_eq!(paths.write(&under, b"y"), Err(through));
    let config = dir.join("config");
    (host.make_dir)(&config);
    paths.write(&config.join("a.hcl"), b"x").unwrap();
    let on_dir = paths.write(&config, b"y");
    assert_eq!(on_dir, Err(Error::Io { code: DIRECTORY }));
    let entries = vec![config, file];
    assert_eq!(paths.read(dir, 0), Ok(Found::Directory(entries)));
}
