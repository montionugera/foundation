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
        make_dir: |_| {},
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
    assert_eq!(format!("{:?}", b.paths()), "Paths(Node(1))");
}
