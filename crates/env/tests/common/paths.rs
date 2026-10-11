//! The cases of `env::paths` that `os` and `sim` both pass. The tests of each include
//! this file by `#[path]`, so it uses only `env` and `std`.

use std::path::{Path, PathBuf};

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
pub(crate) const NOT_DIRECTORY: i32 = 20;
pub(crate) const DIRECTORY: i32 = 21;

/// One `#[test]` for each case, which gives the case to `$run`. Call it where this
/// module is in scope as `cases`.
macro_rules! tests {
    ($run:ident) => {
        cases::tests!(
            @each $run,
            a_file_at_the_bound_reads_whole,
            a_file_over_the_bound_reads_large,
            a_directory_lists_in_name_order,
            a_directory_whose_names_pass_the_bound_reads_large,
            a_path_with_nothing_is_not_found,
            a_write_replaces_the_file,
            a_path_through_a_file_fails,
            a_path_that_ends_in_a_slash_names_a_directory,
            an_empty_path_is_not_found,
        );
    };
    (@each $run:ident, $($case:ident),* $(,)?) => {
        $(
            #[test]
            fn $case() {
                $run(cases::$case);
            }
        )*
    };
}
pub(crate) use tests;

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
        paths.read(&config, 64),
        Ok(Found::Directory(entries.to_vec()))
    );
    assert_eq!(paths.read(dir, 64), Ok(Found::Directory(vec![config])));
}

pub(crate) fn a_directory_whose_names_pass_the_bound_reads_large(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let config = dir.join("config");
    (host.make_dir)(&config);
    for name in ["a.hcl", "b.hcl", "c"] {
        paths.write(&config.join(name), b"x").unwrap();
    }
    let entries = ["a.hcl", "b.hcl", "c"].map(|name| config.join(name));
    let listed = Ok(Found::Directory(entries.to_vec()));
    assert_eq!(paths.read(&config, 11), listed);
    assert_eq!(paths.read(&config, 10), Ok(Found::Large));
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
    assert_eq!(paths.read(dir, 64), Ok(Found::Directory(vec![path])));
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
    assert_eq!(paths.read(dir, 64), Ok(Found::Directory(entries)));
}

pub(crate) fn a_path_that_ends_in_a_slash_names_a_directory(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let file = dir.join("plant.hcl");
    paths.write(&file, b"x").unwrap();
    let slashed = PathBuf::from(format!("{}/", file.display()));
    let through = Error::Io {
        code: NOT_DIRECTORY,
    };
    assert_eq!(paths.read(&slashed, 1), Err(through));
    assert_eq!(paths.write(&slashed, b"y"), Err(through));
    let none = PathBuf::from(format!("{}/", dir.join("none").display()));
    assert_eq!(paths.read(&none, 1), Err(Error::NotFound));
    assert_eq!(paths.write(&none, b"y"), Err(through));
    let config = dir.join("config");
    (host.make_dir)(&config);
    paths.write(&config.join("a.hcl"), b"x").unwrap();
    let slashed = PathBuf::from(format!("{}/", config.display()));
    let entries = vec![slashed.join("a.hcl")];
    assert_eq!(paths.read(&slashed, 64), Ok(Found::Directory(entries)));
    assert_eq!(paths.write(&slashed, b"y"), Err(through));
    assert_eq!(paths.read(&file, 1), Ok(Found::File(b"x".to_vec())));
    let entries = vec![config, file];
    assert_eq!(paths.read(dir, 64), Ok(Found::Directory(entries)));
}

pub(crate) fn an_empty_path_is_not_found(host: &Host<'_>) {
    let paths = host.paths;
    assert_eq!(paths.read(Path::new(""), 0), Err(Error::NotFound));
    assert_eq!(paths.write(Path::new(""), b"x"), Err(Error::NotFound));
}
