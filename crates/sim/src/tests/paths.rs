//! Tests of the files of a node's host through `env::paths`.

#[path = "../../../env/tests/common/paths.rs"]
mod cases;

use std::path::{Path, PathBuf};
use std::slice;

use env::paths::{Driver, Entry, Error, Found};

use crate::{Config, Crash, Sim, node};
use cases::Host;

fn run(case: fn(&Host<'_>)) {
    let node = Sim::new(Config::default()).node(node::Config::default());
    case(&Host {
        paths: &node.paths(),
        dir: Path::new("/"),
        make_dir: &|path| node.make_dir(path),
        make_link: &|path, target| node.link_path(path, target),
        move_path: &|from, to| node.move_path(from, to),
    });
}

cases::tests!(run);

#[test]
fn a_relative_path_is_under_the_root() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/config"));
    paths.write(Path::new("config/../plant.hcl"), b"x").unwrap();
    paths.write(Path::new("./config/a.hcl"), b"y").unwrap();
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(Path::new("/plant.hcl"), 1), file);
    let entries = vec![PathBuf::from("config/a.hcl")];
    assert_eq!(
        paths.read(Path::new("config"), 64),
        Ok(Found::Directory(entries))
    );
    let entries = ["/../config", "/../plant.hcl"].map(PathBuf::from).to_vec();
    assert_eq!(
        paths.read(Path::new("/.."), 64),
        Ok(Found::Directory(entries))
    );
}

#[test]
fn within_checks_a_relative_link_by_its_place() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/w"));
    node.set_working_dir(Path::new("/w"));
    node.link_path(Path::new("/w/to-none"), Path::new("/w/none"));
    let within = node.paths().within(&["/w".into()], &[]).unwrap();
    let made = within.make_dir(Path::new("to-none"));
    assert_eq!(
        made,
        Err(Error::Io {
            code: cases::EXISTS
        })
    );
}

/// The Linux `PATH_MAX` holds 4095 bytes and the end NUL.
#[test]
fn a_path_as_long_as_the_host_takes_reads_and_one_byte_longer_fails() {
    run(|host| cases::a_path_as_long_as_the_host_takes(host, 4095, 36));
}

/// A hidden file that is not there, at a short path through a relative link, whose
/// resolved path is one byte past the Linux `PATH_MAX`.
#[test]
fn within_refuses_a_hidden_name_whose_resolved_path_is_too_long() {
    run(|host| {
        cases::within_refuses_a_hidden_name_whose_resolved_path_is_too_long(
            host, 4095, 36,
        );
    });
}

#[test]
#[should_panic(expected = "the target of /link is too long")]
fn a_link_takes_no_target_that_linux_refuses() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.link_path(Path::new("/link"), Path::new(&"a".repeat(4096)));
}

#[test]
fn a_write_on_the_root_is_on_a_directory() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let on_dir = node.paths().write(Path::new("/"), b"x");
    assert_eq!(
        on_dir,
        Err(Error::Io {
            code: cases::DIRECTORY
        })
    );
}

#[test]
fn a_fault_fails_one_call_with_its_code_and_changes_nothing() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    paths.write(Path::new("/plant.hcl"), b"x").unwrap();
    node.fail_path(Path::new("plant.hcl"), 13);
    node.fail_path(Path::new("/plant.hcl"), 5);
    let path = Path::new("/plant.hcl");
    assert_eq!(paths.write(path, b"y"), Err(Error::Io { code: 13 }));
    assert_eq!(paths.read(path, 1), Err(Error::Io { code: 5 }));
    assert_eq!(paths.read(path, 1), Ok(Found::File(b"x".to_vec())));
}

#[test]
fn a_fault_fails_one_make_of_a_directory_and_makes_nothing() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    let path = Path::new("/data");
    node.fail_path(path, 13);
    assert_eq!(paths.make_dir(path), Err(Error::Io { code: 13 }));
    assert_eq!(paths.read(path, 1), Err(Error::NotFound));
    assert_eq!(paths.make_dir(path), Ok(()));
    assert_eq!(paths.read(path, 1), Ok(Found::Directory(Vec::new())));
}

#[test]
fn each_host_has_its_own_files_and_a_crash_keeps_them() {
    let mut sim = Sim::new(Config::default());
    let (a, b) = (
        sim.node(node::Config::default()),
        sim.node(node::Config::default()),
    );
    a.paths().write(Path::new("/plant.hcl"), b"x").unwrap();
    b.fail_path(Path::new("/plant.hcl"), 5);
    sim.crash(&a, Crash::Power);
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(a.paths().read(Path::new("/plant.hcl"), 1), file);
    let none = b.paths().read(Path::new("/plant.hcl"), 1);
    assert_eq!(none, Err(Error::Io { code: 5 }));
    assert_eq!(
        b.paths().read(Path::new("/plant.hcl"), 1),
        Err(Error::NotFound)
    );
    b.make_dir(Path::new("/config"));
    b.paths().write(Path::new("/config/a.hcl"), b"y").unwrap();
    let entries = vec![PathBuf::from("/plant.hcl")];
    assert_eq!(
        a.paths().read(Path::new("/"), 64),
        Ok(Found::Directory(entries))
    );
    a.paths().write(Path::new("/config"), b"z").unwrap();
}

#[test]
fn the_files_of_a_host_show_its_node() {
    let mut sim = Sim::new(Config::default());
    sim.node(node::Config::default());
    let b = sim.node(node::Config::default());
    assert_eq!(
        format!("{:?}", b.paths()),
        "Paths { driver: Node(1), limits: [] }"
    );
}

#[test]
fn a_relative_path_is_under_the_working_directory_that_a_node_sets() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.set_working_dir(Path::new("/work"));
    assert_eq!(paths.resolve(Path::new(".")), Err(Error::NotFound));
    let missing = paths.write(Path::new("plant.hcl"), b"x");
    assert_eq!(missing, Err(Error::NotFound));
    node.make_dir(Path::new("/work"));
    paths.write(Path::new("plant.hcl"), b"x").unwrap();
    assert_eq!(paths.resolve(Path::new(".")), Ok(PathBuf::from("/work")));
    node.fail_path(Path::new("plant.hcl"), 5);
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(
        paths.read(Path::new("/work/plant.hcl"), 1),
        Err(Error::Io { code: 5 })
    );
    assert_eq!(paths.read(Path::new("/work/plant.hcl"), 1), file);
    let up = paths.resolve(Path::new("../b.hcl"));
    assert_eq!(up, Ok(PathBuf::from("/b.hcl")));
    node.set_working_dir(Path::new("/"));
    assert_eq!(paths.read(Path::new("work/plant.hcl"), 1), file);
}

/// A relative hidden path starts at the working directory of `within`, so it stays
/// when that directory goes or changes, and hides nothing at the new one.
#[test]
fn a_relative_hidden_path_stays_where_the_working_directory_of_within_was() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    for dir in ["/w/sub", "/x/sub"] {
        node.make_dir(Path::new(dir));
    }
    for file in ["/w/secret", "/x/secret"] {
        paths.write(Path::new(file), b"x").unwrap();
    }
    node.set_working_dir(Path::new("/w/sub"));
    let folders = [PathBuf::from("/w"), PathBuf::from("/x")];
    let within = paths
        .within(&folders, &[PathBuf::from("../secret")])
        .unwrap();
    node.move_path(Path::new("/w/sub"), Path::new("/gone"));
    let gone = within.read(Path::new("/w/secret"), 1);
    node.set_working_dir(Path::new("/x/sub"));
    let moved = within.read(Path::new("/w/secret"), 1);
    let other = within.read(Path::new("/x/secret"), 1);
    assert_eq!(
        (gone, moved, other),
        (
            Err(Error::Hidden),
            Err(Error::Hidden),
            Ok(Found::File(b"x".to_vec()))
        )
    );
}

/// The host bounds a relative hidden path as given, not on the working directory.
#[test]
fn a_short_relative_hidden_path_from_a_deep_working_directory_hides() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    let mut deep = PathBuf::from("/w");
    while deep.as_os_str().len() + 201 < 4090 {
        deep.push("d".repeat(200));
    }
    deep.push("e".repeat(4089 - deep.as_os_str().len()));
    node.make_dir(&deep.join("s"));
    paths.write(&deep.join("k"), b"x").unwrap();
    node.set_working_dir(&deep);
    let hidden = PathBuf::from("s/../k");
    let within = paths.within(slice::from_ref(&deep), slice::from_ref(&hidden));
    let read = within.map(|within| within.read(&deep.join("k"), 1));
    assert_eq!(read, Ok(Err(Error::Hidden)));
}

/// `within` refuses a relative hidden path as `resolve` does, before it takes off
/// each `..` at its start.
#[test]
fn within_refuses_a_long_relative_hidden_path_as_resolve_does() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/w/sub"));
    paths.write(Path::new("/k"), b"x").unwrap();
    node.set_working_dir(Path::new("/w/sub"));
    let hidden = PathBuf::from(format!("{}k", "../".repeat(1400)));
    let resolved = paths.resolve(&hidden).err();
    let within = paths
        .within(&[PathBuf::from("/")], slice::from_ref(&hidden))
        .err();
    let too_long = Some(Error::Io { code: 36 });
    assert_eq!((resolved, within), (too_long, too_long));
}

/// Each call walks a relative hidden path from the working directory of `within`, so
/// a link on it that a program changes after `within` hides its new target.
#[test]
fn a_relative_hidden_path_hides_the_new_target_of_a_link_on_it() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    for dir in ["/w/sub", "/w/a", "/w/b"] {
        node.make_dir(Path::new(dir));
    }
    for file in ["/w/a/secret", "/w/b/secret"] {
        paths.write(Path::new(file), b"x").unwrap();
    }
    node.link_path(Path::new("/w/sub/link"), Path::new("/w/a"));
    node.set_working_dir(Path::new("/w"));
    let hidden = [PathBuf::from("sub/link/secret")];
    let within = paths.within(&[PathBuf::from("/w")], &hidden).unwrap();
    node.move_path(Path::new("/w/sub/link"), Path::new("/old"));
    node.link_path(Path::new("/w/sub/link"), Path::new("/w/b"));
    let old = within.read(Path::new("/w/a/secret"), 1);
    let new = within.read(Path::new("/w/b/secret"), 1);
    assert_eq!(
        (old, new),
        (Ok(Found::File(b"x".to_vec())), Err(Error::Hidden))
    );
}

#[test]
fn within_checks_a_relative_path_that_does_not_resolve_from_the_working_directory() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work", "/out"] {
        node.make_dir(Path::new(dir));
    }
    let within = node.paths().within(&[PathBuf::from("/work")], &[]).unwrap();
    let path = Path::new("none/x");
    for (dir, error) in [("/out", Error::Outside), ("/work", Error::NotFound)] {
        node.set_working_dir(Path::new(dir));
        assert_eq!(within.read(path, 1), Err(error), "{dir}");
        assert_eq!(within.write(path, b"x"), Err(error), "{dir}");
    }
}

#[test]
fn within_checks_the_working_directory_as_the_first_step_of_a_relative_path() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work/data", "/out"] {
        node.make_dir(Path::new(dir));
    }
    let within = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/work/data")])
        .unwrap();
    for (dir, path, error) in [
        ("/out", "../work/a", Error::Outside),
        ("/out", "./../work/a", Error::Outside),
        ("/out", "/out/../work/a", Error::Outside),
        ("/work/data", "../a", Error::Hidden),
        ("/work/data", "./../a", Error::Hidden),
        ("/work/data", "/work/data/../a", Error::Hidden),
    ] {
        node.set_working_dir(Path::new(dir));
        let path = Path::new(path);
        assert_eq!(within.write(path, b"x"), Err(error), "{dir} {path:?}");
        assert_eq!(within.read(path, 1), Err(error), "{dir} {path:?}");
    }
    node.set_working_dir(Path::new("/work"));
    assert_eq!(within.write(Path::new("../work/a"), b"x"), Ok(()));
    node.make_dir(Path::new("/out/data"));
    let within = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/out/data")])
        .unwrap();
    node.set_working_dir(Path::new("/out/data"));
    for path in ["a", "./a", "/out/data/a"].map(Path::new) {
        assert_eq!(within.read(path, 1), Err(Error::Outside), "{path:?}");
    }
}

#[test]
fn within_keeps_a_narrow_handle_out_of_a_hidden_working_directory() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work/data", "/work/sub"] {
        node.make_dir(Path::new(dir));
    }
    node.paths().write(Path::new("/work/sub/a"), b"x").unwrap();
    node.link_path(Path::new("/work/data/l"), Path::new("/work/sub"));
    let wide = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/work/data")])
        .unwrap();
    let narrow = wide.within(&[PathBuf::from("/work/sub")], &[]).unwrap();
    node.set_working_dir(Path::new("/work/data"));
    for path in ["l/a", "./l/a", "/work/data/l/a"].map(Path::new) {
        assert_eq!(narrow.read(path, 1), Err(Error::Outside), "{path:?}");
    }
}

#[test]
fn within_refuses_a_path_through_a_hidden_folder_outside_the_folders() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work", "/out/h"] {
        node.make_dir(Path::new(dir));
    }
    node.paths().write(Path::new("/work/a"), b"x").unwrap();
    node.link_path(Path::new("/out/h/l"), Path::new("/work"));
    let within = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/out/h")])
        .unwrap();
    let path = Path::new("/out/h/l/a");
    assert_eq!(within.read(path, 1), Err(Error::Outside));
    assert_eq!(node.paths().read(path, 1).map(|_| ()), Ok(()));
}

#[test]
fn within_follows_a_link_outside_that_names_a_folder() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work", "/out"] {
        node.make_dir(Path::new(dir));
    }
    node.link_path(Path::new("/out/in"), Path::new("/work"));
    let within = node.paths().within(&[PathBuf::from("/work")], &[]).unwrap();
    node.set_working_dir(Path::new("/out"));
    for path in ["/out/in/a", "in/a"].map(Path::new) {
        assert_eq!(within.write(path, b"x"), Ok(()), "{path:?}");
        assert_eq!(within.read(path, 1).map(|_| ()), Ok(()), "{path:?}");
    }
    node.make_dir(Path::new("/work/x"));
    for path in ["/out/in/x/../a", "in/x/../a"].map(Path::new) {
        assert_eq!(within.read(path, 1).map(|_| ()), Ok(()), "{path:?}");
    }
    let path = Path::new("in/../in/a");
    assert_eq!(within.read(path, 1), Err(Error::Outside));
    node.make_dir(Path::new("/out/far"));
    node.link_path(Path::new("/work/y"), Path::new("/out/far"));
    let path = Path::new("/out/in/y/../a");
    assert_eq!(within.read(path, 1), Err(Error::Outside));
}

/// A link target that ends in `/.`, with a name after the link: the end `/.` fails at a
/// file, and that file outside gives `Outside` as a directory or nothing does.
#[test]
fn within_tells_no_file_outside_before_an_end_dot_of_a_link() {
    let got = ["dir", "none", "file"].map(|kind| {
        let node = Sim::new(Config::default()).node(node::Config::default());
        node.make_dir(Path::new("/w"));
        node.make_dir(Path::new("/out"));
        let paths = node.paths();
        match kind {
            "dir" => node.make_dir(Path::new("/out/b")),
            "file" => paths.write(Path::new("/out/b"), b"x").unwrap(),
            _ => {}
        }
        for (link, target) in [("/w/l", "/out/b/."), ("/w/m", "/out/b/")] {
            node.link_path(Path::new(link), Path::new(target));
        }
        let within = paths.within(&["/w".into()], &[]).unwrap();
        ["/w/l/x", "/w/m/x"].map(|path| within.read(Path::new(path), 1).map(|_| ()))
    });
    assert_eq!(got, [[Err(Error::Outside); 2]; 3]);
}

#[test]
fn within_tells_no_name_outside_beside_a_hidden_folder_that_is_not_there() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    for dir in ["/work", "/out/x"] {
        node.make_dir(Path::new(dir));
    }
    node.paths().write(Path::new("/out/b.hcl"), b"x").unwrap();
    let within = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/out/h")])
        .unwrap();
    let got = [
        "/out/x",
        "/out/y",
        "/out/x/a",
        "/out/none/a",
        "/out/b.hcl/a",
    ]
    .map(|path| within.read(Path::new(path), 1).map(|_| ()));
    assert_eq!(got, [Err(Error::Outside); 5]);
}

#[test]
fn within_checks_a_relative_first_name_that_does_not_resolve_at_its_place() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/out"));
    for at in ["/work/loop", "/out/loop"].map(Path::new) {
        node.link_path(at, at);
    }
    let within = node
        .paths()
        .within(&[PathBuf::from("/work")], &[PathBuf::from("/work/data")])
        .unwrap();
    let got = ["/work", "/out"].map(|dir| {
        node.set_working_dir(Path::new(dir));
        let path = Path::new("loop");
        (node.paths().read(path, 1), within.read(path, 1))
    });
    let looped = Err(Error::Io { code: 40 });
    assert_eq!(
        got,
        [
            (looped.clone(), looped.clone()),
            (looped, Err(Error::Outside))
        ]
    );
}

#[test]
fn the_home_is_none_until_a_node_sets_it_and_then_resolves() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    assert_eq!(paths.home(), None);
    node.link_path(Path::new("/home"), Path::new("/users"));
    node.set_home(Path::new("/home/user"));
    assert_eq!(paths.home(), None);
    node.make_dir(Path::new("/users"));
    assert_eq!(paths.home(), Some(PathBuf::from("/users/user")));
}

/// The walk never asks `entry` for a path that ends in `/`, so no `Paths` call
/// reaches this arm.
#[test]
fn an_entry_follows_a_link_before_an_end_in_a_separator_as_lstat_does() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/out"));
    node.link_path(Path::new("/link"), Path::new("/out"));
    let link = node.0.entry(Path::new("/link"));
    assert!(
        matches!(&link, Ok(Entry::Link(_, target)) if target == Path::new("/out")),
        "{link:?}"
    );
    let end = node.0.entry(Path::new("/link/"));
    assert!(matches!(end, Ok(Entry::Directory(_))), "{end:?}");
    assert_eq!(end, node.0.entry(Path::new("/out")));
}

#[test]
fn a_link_to_nothing_resolves_to_its_target_and_reads_once_its_file_is_there() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.link_path(Path::new("/plans/b.plan"), Path::new("../config/b.plan"));
    node.make_dir(Path::new("/config"));
    let link = Path::new("/plans/b.plan");
    assert_eq!(paths.resolve(link), Ok(PathBuf::from("/config/b.plan")));
    assert_eq!(paths.read(link, 1), Err(Error::NotFound));
    assert_eq!(paths.write(link, b"x"), Err(Error::NotFound));
    paths.write(Path::new("/config/b.plan"), b"x").unwrap();
    assert_eq!(paths.resolve(link), Ok(PathBuf::from("/config/b.plan")));
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(link, 1), file);
    let entries = ["/config", "/plans"].map(PathBuf::from).to_vec();
    assert_eq!(
        paths.read(Path::new("/"), 64),
        Ok(Found::Directory(entries))
    );
}

#[test]
fn a_path_through_more_than_40_links_fails_with_a_loop() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    for link in 0..41 {
        let path = PathBuf::from(format!("/{link}"));
        node.link_path(&path, Path::new(&(link + 1).to_string()));
    }
    paths.write(Path::new("/41"), b"x").unwrap();
    assert_eq!(paths.resolve(Path::new("/1")), Ok(PathBuf::from("/41")));
    let looped = Error::Io { code: 40 };
    assert_eq!(paths.resolve(Path::new("/0")), Err(looped));
    assert_eq!(paths.read(Path::new("/0"), 1), Err(looped));
    assert_eq!(paths.write(Path::new("/0"), b"x"), Err(looped));
    node.link_path(Path::new("/self"), Path::new("self"));
    assert_eq!(paths.resolve(Path::new("/self/a")), Err(looped));
}

#[test]
fn two_dots_fail_after_a_file_or_a_name_that_is_not_there() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    paths.write(Path::new("/plant.hcl"), b"x").unwrap();
    let through = Error::Io {
        code: cases::NOT_DIRECTORY,
    };
    assert_eq!(paths.resolve(Path::new("/plant.hcl/..")), Err(through));
    assert_eq!(paths.read(Path::new("/plant.hcl/.."), 64), Err(through));
    let missing = Path::new("/none/x/../a");
    assert_eq!(paths.resolve(missing), Err(Error::NotFound));
    assert_eq!(paths.write(missing, b"x"), Err(Error::NotFound));
    let entries = vec![PathBuf::from("/plant.hcl")];
    assert_eq!(
        paths.read(Path::new("/"), 64),
        Ok(Found::Directory(entries))
    );
}

#[test]
fn a_working_directory_through_a_link_resolves_the_link() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    paths.write(Path::new("/target/a.hcl"), b"x").unwrap();
    node.link_path(Path::new("/w"), Path::new("/target"));
    node.set_working_dir(Path::new("/w"));
    assert_eq!(paths.resolve(Path::new(".")), Ok(PathBuf::from("/target")));
    let a = paths.resolve(Path::new("a.hcl"));
    assert_eq!(a, Ok(PathBuf::from("/target/a.hcl")));
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(Path::new("a.hcl"), 1), file);
    paths.write(Path::new("b.hcl"), b"y").unwrap();
    let file = Ok(Found::File(b"y".to_vec()));
    assert_eq!(paths.read(Path::new("/w/b.hcl"), 1), file);
    assert_eq!(paths.read(Path::new("/target/b.hcl"), 1), file);
}

#[test]
fn a_link_under_a_link_is_reached_through_it() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    paths.write(Path::new("/target/a.hcl"), b"x").unwrap();
    node.link_path(Path::new("/w"), Path::new("/target"));
    node.link_path(Path::new("/w/b.hcl"), Path::new("a.hcl"));
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(Path::new("/w/b.hcl"), 1), file);
    assert_eq!(paths.read(Path::new("/target/b.hcl"), 1), file);
}

#[test]
fn a_directory_that_a_node_makes_is_there_with_no_entry() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.link_path(Path::new("/w"), Path::new("/target"));
    node.make_dir(Path::new("/target"));
    node.make_dir(Path::new("/w/a"));
    node.make_dir(Path::new("/w/a"));
    let empty = Ok(Found::Directory(vec![]));
    assert_eq!(paths.read(Path::new("/target/a"), 0), empty);
    let missing = paths.resolve(Path::new("/target/a/none.plan"));
    assert_eq!(missing, Ok(PathBuf::from("/target/a/none.plan")));
    let directory = Err(Error::Io {
        code: cases::DIRECTORY,
    });
    assert_eq!(paths.write(Path::new("/target/a"), b"x"), directory);
    paths.write(Path::new("/target/a/b.hcl"), b"x").unwrap();
    let entries = Ok(Found::Directory(vec![PathBuf::from("/w/a/b.hcl")]));
    assert_eq!(paths.read(Path::new("/w/a"), 64), entries);
}

#[test]
fn a_relative_directory_is_made_under_the_working_directory() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/w"));
    node.set_working_dir(Path::new("/w"));
    node.link_path(Path::new("/w/to-none"), Path::new("none"));
    let exists = Err(Error::Io {
        code: cases::EXISTS,
    });
    assert_eq!(paths.make_dir(Path::new("to-none")), exists);
    assert_eq!(paths.make_dir(Path::new("new")), Ok(()));
    let empty = Ok(Found::Directory(vec![]));
    assert_eq!(paths.read(Path::new("/w/new"), 0), empty);
}

/// The hidden folder keeps its key, so it hides what it holds by its new name too.
#[test]
fn a_moved_directory_keeps_its_key_and_what_it_holds() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/w/data"));
    paths.write(Path::new("/w/data/admin.key"), b"x").unwrap();
    let within = paths.within(&["/w".into()], &["/w/data".into()]).unwrap();
    node.move_path(Path::new("/w/data"), Path::new("/w/old"));
    let entries = Ok(Found::Directory(vec![PathBuf::from("/w/old/admin.key")]));
    assert_eq!(paths.read(Path::new("/w/old"), 64), entries);
    let read = within.read(Path::new("/w/old/admin.key"), 1);
    assert_eq!(read, Err(Error::Hidden));
    assert_eq!(paths.read(Path::new("/w/data"), 0), Err(Error::NotFound));
}

/// A host keeps a process on its working directory when a program outside moves it,
/// so a relative path resolves in the moved directory, the folder that `within` took.
#[test]
fn a_moved_working_directory_stays_the_working_directory() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/w/work"));
    node.set_working_dir(Path::new("/w/work"));
    let within = paths.within(&["/w/work".into()], &[]).unwrap();
    node.move_path(Path::new("/w"), Path::new("/old"));
    node.make_dir(Path::new("/w/work"));
    let resolved = paths.resolve(Path::new("."));
    let written = within.write(Path::new("out.plan"), b"x");
    assert_eq!(
        (resolved, written),
        (Ok(PathBuf::from("/old/work")), Err(Error::Replaced))
    );
}

/// A host keeps a process on the directory that a link led to, so a move of the link,
/// or of the directory that holds it, does not move the working directory.
#[test]
fn a_working_directory_through_a_moved_link_stays() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target/sub"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/w/link/sub"));
    node.move_path(Path::new("/w"), Path::new("/old"));
    assert_eq!(
        paths.resolve(Path::new(".")),
        Ok(PathBuf::from("/target/sub"))
    );
    node.link_path(Path::new("/a/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/a/link/sub"));
    node.move_path(Path::new("/a/link"), Path::new("/a/moved"));
    assert_eq!(
        paths.resolve(Path::new(".")),
        Ok(PathBuf::from("/target/sub"))
    );
}

/// The working directory moves with the target of a link on its way.
#[test]
fn a_working_directory_moves_with_the_target_of_a_link() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target/sub"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/w/link/sub"));
    node.move_path(Path::new("/target"), Path::new("/moved"));
    assert_eq!(
        paths.resolve(Path::new(".")),
        Ok(PathBuf::from("/moved/sub"))
    );
}

#[test]
fn a_working_directory_at_a_moved_link_stays() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    node.make_dir(Path::new("/w"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/w/link"));
    let within = paths.within(&["/target".into()], &[]).unwrap();
    node.move_path(Path::new("/w/link"), Path::new("/w/moved"));
    let resolved = paths.resolve(Path::new("."));
    let written = within.write(Path::new("out.plan"), b"x");
    assert_eq!((resolved, written), (Ok(PathBuf::from("/target")), Ok(())));
}

#[test]
fn a_fault_on_a_relative_path_holds_through_a_move_elsewhere() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    node.make_dir(Path::new("/w"));
    node.make_dir(Path::new("/x"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/w/link"));
    node.fail_path(Path::new("out"), 5);
    node.move_path(Path::new("/x"), Path::new("/y"));
    assert_eq!(
        paths.write(Path::new("out"), b"x"),
        Err(Error::Io { code: 5 })
    );
}

#[test]
fn a_fault_on_a_relative_path_moves_with_the_working_directory_in_turn() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    node.set_working_dir(Path::new("/target"));
    node.fail_path(Path::new("out"), 5);
    node.fail_path(Path::new("/t2/out"), 13);
    node.move_path(Path::new("/target"), Path::new("/t2"));
    let written = [Path::new("/t2/out"), Path::new("out"), Path::new("out")]
        .map(|path| paths.write(path, b"x"));
    let failed = [
        Err(Error::Io { code: 5 }),
        Err(Error::Io { code: 13 }),
        Ok(()),
    ];
    assert_eq!(written, failed);
}

#[test]
fn a_move_elsewhere_keeps_a_fault_on_the_working_directory_as_spelled() {
    let written = [false, true].map(|moved| {
        let node = Sim::new(Config::default()).node(node::Config::default());
        let paths = node.paths();
        node.make_dir(Path::new("/target"));
        node.make_dir(Path::new("/x"));
        node.link_path(Path::new("/w/link"), Path::new("/target"));
        node.set_working_dir(Path::new("/w/link"));
        node.fail_path(Path::new("/w/link/out"), 5);
        if moved {
            node.move_path(Path::new("/x"), Path::new("/y"));
        }
        paths.write(Path::new("out"), b"x")
    });
    assert_eq!(
        written,
        [Err(Error::Io { code: 5 }); 2],
        "no move, move of /x"
    );
}

#[test]
fn a_fault_follows_no_link_to_match_a_call() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.set_working_dir(Path::new("/w/link"));
    node.fail_path(Path::new("out"), 5);
    node.fail_path(Path::new("/target/out"), 13);
    let written = [Path::new("/target/out"), Path::new("out")]
        .map(|path| paths.write(path, b"x"));
    assert_eq!(
        written,
        [Err(Error::Io { code: 13 }), Err(Error::Io { code: 5 })]
    );
}

#[test]
fn a_moved_link_names_its_target_and_a_link_on_the_way_is_followed() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.make_dir(Path::new("/target"));
    node.link_path(Path::new("/w"), Path::new("/target"));
    node.link_path(Path::new("/w/link"), Path::new("/target"));
    node.move_path(Path::new("/w/link"), Path::new("/w/moved"));
    let moved = paths.resolve(Path::new("/target/moved"));
    assert_eq!(moved, Ok(PathBuf::from("/target")));
    assert_eq!(paths.resolve(Path::new("/w")), Ok(PathBuf::from("/target")));
}

#[test]
#[should_panic(expected = "nothing is at /none")]
fn a_move_of_nothing_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.move_path(Path::new("/none"), Path::new("/to"));
}

#[test]
#[should_panic(expected = "/b is there")]
fn a_move_onto_a_directory_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/a"));
    node.make_dir(Path::new("/b"));
    node.move_path(Path::new("/a"), Path::new("/b"));
}

#[test]
#[should_panic(expected = "/none is not a directory")]
fn a_move_into_a_directory_that_is_not_there_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/a"));
    node.move_path(Path::new("/a"), Path::new("/none/a"));
}

#[test]
#[should_panic(expected = "/a/b is under /a")]
fn a_move_under_itself_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/a"));
    node.move_path(Path::new("/a"), Path::new("/a/b"));
}

#[test]
#[should_panic(expected = "/ has no directory")]
fn a_move_of_the_root_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.move_path(Path::new("/"), Path::new("/a"));
}

#[test]
#[should_panic(expected = "a is relative")]
fn a_relative_move_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/b"));
    node.move_path(Path::new("/b"), Path::new("a"));
}

#[test]
#[should_panic(expected = "/b/../a has a `..` segment")]
fn a_move_with_two_dots_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.move_path(Path::new("/b/../a"), Path::new("/a"));
}

#[test]
#[should_panic(expected = "/plant.hcl/a gave Io { code: 20 }")]
fn a_link_through_a_file_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.paths().write(Path::new("/plant.hcl"), b"x").unwrap();
    node.link_path(Path::new("/plant.hcl/a"), Path::new("/b"));
}

#[test]
#[should_panic(expected = "/plant.hcl/a gave Io { code: 20 }")]
fn a_directory_through_a_file_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.paths().write(Path::new("/plant.hcl"), b"x").unwrap();
    node.make_dir(Path::new("/plant.hcl/a"));
}

#[test]
#[should_panic(expected = "/w/a gave NotFound")]
fn a_directory_through_a_link_to_nothing_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.link_path(Path::new("/w"), Path::new("/target"));
    node.make_dir(Path::new("/w/a"));
}

#[test]
#[should_panic(expected = "/plant.hcl is there")]
fn a_directory_on_a_file_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.paths().write(Path::new("/plant.hcl"), b"x").unwrap();
    node.make_dir(Path::new("/plant.hcl"));
}

#[test]
#[should_panic(expected = "/link is there")]
fn a_directory_on_a_link_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.link_path(Path::new("/link"), Path::new("/b"));
    node.make_dir(Path::new("/link"));
}

#[test]
#[should_panic(expected = "/ is there")]
fn a_directory_at_the_root_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/"));
}

#[test]
#[should_panic(expected = "dir is relative")]
fn a_relative_directory_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("dir"));
}

#[test]
#[should_panic(expected = "/a/../dir has a `..` segment")]
fn a_directory_with_two_dots_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/a/../dir"));
}

#[test]
#[should_panic(expected = "/plant.hcl is there")]
fn a_link_on_a_file_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.paths().write(Path::new("/plant.hcl"), b"x").unwrap();
    node.link_path(Path::new("/plant.hcl"), Path::new("b.hcl"));
}

#[test]
#[should_panic(expected = "/config is there")]
fn a_link_on_a_directory_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.make_dir(Path::new("/config"));
    node.link_path(Path::new("/config"), Path::new("b"));
}

#[test]
#[should_panic(expected = "link is relative")]
fn a_relative_link_path_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.link_path(Path::new("link"), Path::new("b"));
}

#[test]
#[should_panic(expected = "/a/../link has a `..` segment")]
fn a_link_path_with_two_dots_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.link_path(Path::new("/a/../link"), Path::new("b"));
}

#[test]
#[should_panic(expected = "work is relative")]
fn a_relative_working_directory_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.set_working_dir(Path::new("work"));
}

#[test]
#[should_panic(expected = "/a/../work has a `..` segment")]
fn a_working_directory_with_two_dots_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.set_working_dir(Path::new("/a/../work"));
}

#[test]
#[should_panic(expected = "home is relative")]
fn a_relative_home_panics() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    node.set_home(Path::new("home"));
}
