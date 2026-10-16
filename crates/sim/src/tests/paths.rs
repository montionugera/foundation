//! Tests of the files of a node's host through `env::paths`.

#[path = "../../../env/tests/common/paths.rs"]
mod cases;

use std::path::{Path, PathBuf};

use env::paths::{Error, Found};

use crate::{Config, Crash, Sim, node};
use cases::Host;

fn run(case: fn(&Host<'_>)) {
    let node = Sim::new(Config::default()).node(node::Config::default());
    case(&Host {
        paths: &node.paths(),
        dir: Path::new("/"),
        make_dir: &|path| node.make_dir(path),
        make_link: &|path, target| node.link_path(path, target),
    });
}

cases::tests!(run);

#[test]
fn a_relative_path_is_under_the_root() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
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

#[test]
fn a_write_through_a_link_to_nothing_makes_the_file_it_names() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
    node.link_path(Path::new("/plans/b.plan"), Path::new("../config/b.plan"));
    let resolved = paths.resolve(Path::new("/plans/b.plan"));
    assert_eq!(resolved, Err(Error::NotFound));
    paths.write(Path::new("/plans/b.plan"), b"x").unwrap();
    let resolved = paths.resolve(Path::new("/plans/b.plan"));
    assert_eq!(resolved, Ok(PathBuf::from("/config/b.plan")));
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(Path::new("/config/b.plan"), 1), file);
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
fn two_dots_after_a_file_fail_and_in_a_write_after_a_name_that_is_not_there_take_it_off()
 {
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
    paths.write(missing, b"x").unwrap();
    let file = Ok(Found::File(b"x".to_vec()));
    assert_eq!(paths.read(Path::new("/none/a"), 1), file);
}

#[test]
fn a_working_directory_through_a_link_resolves_the_link() {
    let node = Sim::new(Config::default()).node(node::Config::default());
    let paths = node.paths();
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
    node.paths()
        .write(Path::new("/config/a.hcl"), b"x")
        .unwrap();
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
