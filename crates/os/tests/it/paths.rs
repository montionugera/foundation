//! `os::paths` on the real files of the host.

use std::path::Path;

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
    std::fs::create_dir(path).unwrap();
}

macro_rules! cases {
    ($($case:ident),* $(,)?) => {
        $(
            #[test]
            fn $case() {
                run(cases::$case);
            }
        )*
    };
}

cases!(
    a_file_at_the_bound_reads_whole,
    a_file_over_the_bound_reads_large,
    a_directory_lists_in_name_order,
    a_path_with_nothing_is_not_found,
    a_write_replaces_the_file,
    a_path_through_a_file_fails,
);

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
