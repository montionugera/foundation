//! Builds a probe crate with the `build.rs` of `ops` in a fresh repository, and checks
//! that the commit it prints follows `HEAD` after each git step.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = include_str!("../build.rs");

/// Runs `program` in `dir` with `env`, and gives its trimmed output. Panics when it fails.
fn run(dir: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("FOUNDATION_COMMIT")
        .envs(env.iter().copied())
        .output()
        .expect("spawn");
    assert!(
        output.status.success(),
        "{program} {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    run(dir, "git", args, &[])
}

/// Makes a probe crate with the `build.rs` of `ops` in a new repository at `name`,
/// with `init` as more arguments of `git init`.
fn create_probe(name: &str, init: &[&str]) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("commit")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .expect("write");
    std::fs::write(root.join("build.rs"), SCRIPT).expect("write");
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() {\n    let commit = env!(\"FOUNDATION_COMMIT\");\n    \
         println!(\"{}\", if commit.is_empty() { \"none\" } else { commit });\n}\n",
    )
    .expect("write");
    std::fs::write(root.join(".gitignore"), "target\n").expect("write");
    let mut args = vec!["init", "-q", "-b", "main"];
    args.extend_from_slice(init);
    git(&root, &args);
    git(&root, &["config", "user.name", "probe"]);
    git(&root, &["config", "user.email", "probe@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    root
}

/// Builds and runs the probe at `root`, and gives the commit it prints.
fn built(root: &Path) -> String {
    built_with(root, &[])
}

/// Builds and runs the probe at `root` with `env`, and gives the commit it prints.
fn built_with(root: &Path, env: &[(&str, &str)]) -> String {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let target = root.join("target");
    let target = target.to_str().expect("utf8");
    run(
        root,
        &cargo,
        &["run", "-q", "--offline", "--target-dir", target],
        env,
    )
}

/// Commits at `root`, and gives the commit.
fn commit(root: &Path) -> String {
    git(root, &["commit", "-q", "--allow-empty", "-m", "c"]);
    git(root, &["rev-parse", "HEAD"])
}

#[test]
fn a_first_commit_after_a_build_on_an_unborn_branch_rebuilds() {
    let root = create_probe("unborn", &[]);
    assert_eq!(built(&root), "none");
    let head = commit(&root);
    assert_eq!(built(&root), head);
}

#[test]
fn a_commit_on_an_orphan_branch_rebuilds() {
    let root = create_probe("orphan", &[]);
    commit(&root);
    assert_eq!(built(&root), git(&root, &["rev-parse", "HEAD"]));
    git(&root, &["checkout", "-q", "--orphan", "other"]);
    assert_eq!(built(&root), "none");
    let head = commit(&root);
    assert_eq!(built(&root), head);
}

#[test]
fn a_commit_in_a_reftable_repository_rebuilds() {
    let root = create_probe("reftable", &["--ref-format=reftable"]);
    let first = commit(&root);
    assert_eq!(built(&root), first);
    let second = commit(&root);
    assert_eq!(built(&root), second);
}

#[test]
fn a_commit_on_a_packed_nested_branch_rebuilds() {
    let root = create_probe("packed", &[]);
    commit(&root);
    git(&root, &["checkout", "-q", "-b", "a/b"]);
    git(&root, &["pack-refs", "--all"]);
    let first = git(&root, &["rev-parse", "HEAD"]);
    assert_eq!(built(&root), first);
    let second = commit(&root);
    assert_eq!(built(&root), second);
}

#[test]
fn a_detached_checkout_rebuilds() {
    let root = create_probe("detached", &[]);
    let first = commit(&root);
    let second = commit(&root);
    assert_eq!(built(&root), second);
    git(&root, &["checkout", "-q", "--detach", &first]);
    assert_eq!(built(&root), first);
}

#[test]
fn a_variable_of_the_builder_gives_no_commit() {
    let root = create_probe("variable", &[]);
    commit(&root);
    let env = [("FOUNDATION_COMMIT", "bogus"), ("GIT_DIR", "/nonexistent")];
    assert_eq!(built_with(&root, &env), "none");
}
