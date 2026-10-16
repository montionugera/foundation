//! The cases of `env::paths` that `os` and `sim` both pass. The tests of each include
//! this file by `#[path]`, so it uses only `env` and `std`.

use std::path::{Path, PathBuf};
use std::slice;

use env::paths::{Error, Found, Paths};

/// What a case runs on.
pub(crate) struct Host<'a> {
    pub(crate) paths: &'a Paths,
    /// A directory that is there and empty. On `sim`, only `/` is there with no file
    /// under it.
    pub(crate) dir: &'a Path,
    /// Makes a directory in a directory that is there.
    pub(crate) make_dir: &'a dyn Fn(&Path),
    /// Makes a symbolic link at the first path that names the second.
    pub(crate) make_link: &'a dyn Fn(&Path, &Path),
}

/// `ENOTDIR` and `EISDIR` on Linux and macOS.
pub(crate) const NOT_DIRECTORY: i32 = 20;
pub(crate) const DIRECTORY: i32 = 21;

/// A bound that each directory of a case passes.
const ALL: usize = 1 << 12;

/// One `#[test]` for each case, which gives the case to `$run`. Call it where this
/// module is in scope as `cases`.
macro_rules! tests {
    ($run:ident) => {
        cases::tests!(
            @each $run,
            a_file_at_the_bound_reads_whole,
            a_file_over_the_bound_reads_large,
            a_directory_lists_in_name_order,
            a_directory_whose_paths_pass_the_bound_reads_large,
            a_path_with_nothing_is_not_found,
            a_write_replaces_the_file,
            a_path_through_a_file_fails,
            a_path_that_ends_in_a_slash_or_a_dot_names_a_directory,
            an_empty_path_is_not_found,
            a_resolve_gives_the_absolute_path_with_no_dots,
            a_resolve_keeps_a_last_name_that_is_not_there,
            a_resolve_follows_each_link,
            a_resolve_through_a_file_fails,
            a_resolve_through_a_directory_that_is_not_there_is_not_found,
            a_resolve_of_a_link_to_nothing_is_not_found,
            within_reaches_only_its_folders,
            within_follows_a_link_out,
            within_resolves_its_folders,
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
        paths.read(&config, ALL),
        Ok(Found::Directory(entries.to_vec()))
    );
    assert_eq!(paths.read(dir, ALL), Ok(Found::Directory(vec![config])));
}

pub(crate) fn a_directory_whose_paths_pass_the_bound_reads_large(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let config = dir.join("config");
    (host.make_dir)(&config);
    for name in ["a.hcl", "b.hcl", "c"] {
        paths.write(&config.join(name), b"x").unwrap();
    }
    let entries = ["a.hcl", "b.hcl", "c"].map(|name| config.join(name));
    let size = entries.iter().map(|entry| entry.as_os_str().len()).sum();
    let listed = Ok(Found::Directory(entries.to_vec()));
    assert_eq!(paths.read(&config, size), listed);
    assert_eq!(paths.read(&config, size - 1), Ok(Found::Large));
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
    assert_eq!(paths.read(&path, ALL), Ok(Found::File(b"second".to_vec())));
    assert_eq!(paths.read(dir, ALL), Ok(Found::Directory(vec![path])));
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
    assert_eq!(paths.read(dir, ALL), Ok(Found::Directory(entries)));
}

pub(crate) fn a_path_that_ends_in_a_slash_or_a_dot_names_a_directory(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let file = dir.join("plant.hcl");
    paths.write(&file, b"x").unwrap();
    let config = dir.join("config");
    (host.make_dir)(&config);
    paths.write(&config.join("a.hcl"), b"x").unwrap();
    let through = Error::Io {
        code: NOT_DIRECTORY,
    };
    for end in ["/", "/."] {
        let named = |path: &Path| PathBuf::from(format!("{}{end}", path.display()));
        assert_eq!(paths.read(&named(&file), 1), Err(through), "{end}");
        assert_eq!(paths.write(&named(&file), b"y"), Err(through), "{end}");
        let none = named(&dir.join("none"));
        assert_eq!(paths.read(&none, 1), Err(Error::NotFound), "{end}");
        assert_eq!(paths.write(&none, b"y"), Err(Error::NotFound), "{end}");
        let named = named(&config);
        let entries = Ok(Found::Directory(vec![named.join("a.hcl")]));
        assert_eq!(paths.read(&named, ALL), entries, "{end}");
        let on_dir = Err(Error::Io { code: DIRECTORY });
        assert_eq!(paths.write(&named, b"y"), on_dir, "{end}");
    }
    let dots = file.join("x/..");
    assert_eq!(paths.read(&dots, 1), Err(through));
    assert_eq!(paths.write(&dots, b"y"), Err(through));
    let none = dir.join("none/x/..");
    assert_eq!(paths.write(&none, b"y"), Err(Error::NotFound));
    assert_eq!(paths.read(&file, 1), Ok(Found::File(b"x".to_vec())));
    let entries = vec![config, file];
    assert_eq!(paths.read(dir, ALL), Ok(Found::Directory(entries)));
}

pub(crate) fn an_empty_path_is_not_found(host: &Host<'_>) {
    let paths = host.paths;
    assert_eq!(paths.read(Path::new(""), 0), Err(Error::NotFound));
    assert_eq!(paths.write(Path::new(""), b"x"), Err(Error::NotFound));
}

pub(crate) fn a_resolve_gives_the_absolute_path_with_no_dots(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let base = paths.resolve(dir).unwrap();
    assert!(base.is_absolute(), "{}", base.display());
    let config = dir.join("config");
    (host.make_dir)(&config);
    (host.make_dir)(&config.join("c"));
    paths.write(&config.join("a.hcl"), b"x").unwrap();
    let a = base.join("config/a.hcl");
    for path in ["config/a.hcl", "config/./a.hcl", "./config/../config/a.hcl"] {
        assert_eq!(paths.resolve(&dir.join(path)), Ok(a.clone()), "{path}");
    }
    for path in ["config", "config/", "config/.", "config/c/.."] {
        let resolved = paths.resolve(&dir.join(path));
        assert_eq!(resolved, Ok(base.join("config")), "{path}");
    }
    assert_eq!(paths.resolve(&dir.join("config/..")), Ok(base));
}

pub(crate) fn a_resolve_keeps_a_last_name_that_is_not_there(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let base = paths.resolve(dir).unwrap();
    (host.make_dir)(&dir.join("config"));
    for path in ["none", "none/", "config/none.plan", "config/../none"] {
        let resolved = paths.resolve(&dir.join(path)).unwrap();
        let name = Path::new(path).file_name().unwrap();
        assert_eq!(resolved.file_name(), Some(name), "{path}");
        assert!(resolved.starts_with(&base), "{path}");
    }
    assert_eq!(paths.read(&dir.join("none"), 1), Err(Error::NotFound));
}

pub(crate) fn a_resolve_follows_each_link(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let base = paths.resolve(dir).unwrap();
    let (config, plans) = (dir.join("config"), dir.join("plans"));
    (host.make_dir)(&config);
    (host.make_dir)(&plans);
    paths.write(&config.join("a.hcl"), b"x").unwrap();
    (host.make_link)(&dir.join("to-config"), &config);
    (host.make_link)(&plans.join("up"), Path::new(".."));
    (host.make_link)(&plans.join("a.hcl"), Path::new("../config/a.hcl"));
    let a = base.join("config/a.hcl");
    for path in ["to-config/a.hcl", "plans/a.hcl", "plans/up/config/a.hcl"] {
        assert_eq!(paths.resolve(&dir.join(path)), Ok(a.clone()), "{path}");
    }
    let above = paths.resolve(&dir.join("to-config/.."));
    assert_eq!(above, Ok(base.clone()));
    let new = paths.resolve(&dir.join("plans/up/to-config/none.plan"));
    assert_eq!(new, Ok(base.join("config/none.plan")));
    assert_eq!(
        paths.read(&dir.join("plans/a.hcl"), 1),
        Ok(Found::File(b"x".to_vec()))
    );
}

pub(crate) fn a_resolve_through_a_file_fails(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let file = dir.join("plant.hcl");
    paths.write(&file, b"x").unwrap();
    let through = Err(Error::Io {
        code: NOT_DIRECTORY,
    });
    for path in ["plant.hcl/a", "plant.hcl/a/b", "plant.hcl/", "plant.hcl/."] {
        assert_eq!(paths.resolve(&dir.join(path)), through, "{path}");
    }
    assert_eq!(paths.resolve(Path::new("")), Err(Error::NotFound));
}

pub(crate) fn a_resolve_through_a_directory_that_is_not_there_is_not_found(
    host: &Host<'_>,
) {
    let (paths, dir) = (host.paths, host.dir);
    for path in ["none/a", "none/x/..", "none/x/a", "none/./a"] {
        let resolved = paths.resolve(&dir.join(path));
        assert_eq!(resolved, Err(Error::NotFound), "{path}");
        assert_eq!(
            paths.read(&dir.join(path), 1),
            Err(Error::NotFound),
            "{path}"
        );
    }
}

pub(crate) fn a_resolve_of_a_link_to_nothing_is_not_found(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    (host.make_link)(&dir.join("link.plan"), &dir.join("plant.plan"));
    (host.make_link)(&dir.join("next.plan"), &dir.join("link.plan"));
    for name in ["link.plan", "next.plan"] {
        let resolved = paths.resolve(&dir.join(name));
        assert_eq!(resolved, Err(Error::NotFound), "{name}");
    }
}

/// `work` is the folder, `work/data` is hidden, and `workshop` is beside `work`.
fn within_work(host: &Host<'_>) -> (Paths, PathBuf) {
    let (paths, dir) = (host.paths, host.dir);
    for name in ["work", "work/data", "workshop"] {
        (host.make_dir)(&dir.join(name));
    }
    for name in [
        "work/a.hcl",
        "work/data/admin.key",
        "workshop/a.hcl",
        "b.hcl",
    ] {
        paths.write(&dir.join(name), b"x").unwrap();
    }
    let base = paths.resolve(dir).unwrap();
    let within = paths.within(&[base.join("work")], &[base.join("work/data")]);
    (within.unwrap(), base)
}

pub(crate) fn within_reaches_only_its_folders(host: &Host<'_>) {
    let dir = host.dir;
    let (within, base) = within_work(host);
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(within.read(&dir.join("work/a.hcl"), 1), file);
    assert_eq!(within.read(&base.join("work/./a.hcl"), 1), file);
    within.write(&dir.join("work/new.plan"), b"y").unwrap();
    let entries = ["a.hcl", "data", "new.plan"].map(|name| dir.join("work").join(name));
    let listed = Ok(Found::Directory(entries.to_vec()));
    assert_eq!(within.read(&dir.join("work"), ALL), listed);
    for path in [
        "b.hcl",
        "work/../b.hcl",
        "workshop/a.hcl",
        "work/data",
        "work/data/admin.key",
        "work/data/../data/admin.key",
        "none.plan",
    ] {
        let path = dir.join(path);
        assert_eq!(
            within.read(&path, 1),
            Err(Error::Outside),
            "{}",
            path.display()
        );
        let written = within.write(&path, b"y");
        assert_eq!(written, Err(Error::Outside), "{}", path.display());
    }
    assert_eq!(host.paths.read(&dir.join("b.hcl"), 1), file);
    assert_eq!(
        host.paths.read(&dir.join("none.plan"), 1),
        Err(Error::NotFound)
    );
    assert_eq!(host.paths.read(&dir.join("work/data/admin.key"), 1), file);
}

pub(crate) fn within_follows_a_link_out(host: &Host<'_>) {
    let dir = host.dir;
    let (within, _) = within_work(host);
    (host.make_link)(&dir.join("work/out"), &dir.join("workshop"));
    (host.make_link)(&dir.join("work/b.hcl"), Path::new("../b.hcl"));
    (host.make_link)(&dir.join("work/key"), Path::new("data/admin.key"));
    for path in [
        "work/out/a.hcl",
        "work/out/new.plan",
        "work/b.hcl",
        "work/key",
    ] {
        let path = dir.join(path);
        assert_eq!(
            within.read(&path, 1),
            Err(Error::Outside),
            "{}",
            path.display()
        );
        let written = within.write(&path, b"y");
        assert_eq!(written, Err(Error::Outside), "{}", path.display());
    }
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(host.paths.read(&dir.join("b.hcl"), 1), file);
    assert_eq!(
        host.paths.read(&dir.join("workshop/new.plan"), 1),
        Err(Error::NotFound)
    );
}

pub(crate) fn within_resolves_its_folders(host: &Host<'_>) {
    let (paths, dir) = (host.paths, host.dir);
    let (_, base) = within_work(host);
    (host.make_link)(&dir.join("alias"), &dir.join("work"));
    let working = paths.resolve(Path::new(".")).unwrap();
    let up = working.components().skip(1).map(|_| Path::new(".."));
    let relative: PathBuf = up.collect();
    let relative = relative.join(base.strip_prefix("/").unwrap());
    let key = dir.join("work/data/admin.key");
    let work = [dir.join("work")];
    for hidden in [relative.join("work/data"), dir.join("alias/data")] {
        let within = paths.within(&work, slice::from_ref(&hidden)).unwrap();
        let read = within.read(&key, 1);
        assert_eq!(read, Err(Error::Outside), "{}", hidden.display());
        let written = within.write(&key, b"y");
        assert_eq!(written, Err(Error::Outside), "{}", hidden.display());
    }
    let none = [dir.join("none/work")];
    assert_eq!(paths.within(&none, &[]).err(), Some(Error::NotFound));
    assert_eq!(paths.within(&work, &none).err(), Some(Error::NotFound));
    let file = [dir.join("b.hcl/data")];
    let through = Some(Error::Io {
        code: NOT_DIRECTORY,
    });
    assert_eq!(paths.within(&work, &file).err(), through);
}
