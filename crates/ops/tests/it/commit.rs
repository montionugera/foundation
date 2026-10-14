//! Builds a probe crate with the `build.rs` of `ops` in a fresh repository, and checks
//! that the commit it prints follows `HEAD` after each git step.

use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = include_str!("../../build.rs");

/// Runs `program` in `dir` with `env`, and gives its trimmed output. Panics when it
/// fails.
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
    match std::fs::remove_dir_all(&root) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            panic!("remove {}: {error}", root.display())
        }
        _ => {}
    }
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n\
         [workspace]\n",
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
    let target = root.join("target");
    let target = target.to_str().expect("utf8");
    run(
        root,
        env!("CARGO"),
        &["run", "-q", "--offline", "--target-dir", target],
        env,
    )
}

/// Whether a build of the probe at `root` does nothing, as cargo finds it fresh.
fn fresh(root: &Path) -> bool {
    let target = root.join("target");
    let output = Command::new(env!("CARGO"))
        .args(["build", "-v", "--offline", "--target-dir"])
        .arg(&target)
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "cargo build: {stderr}");
    stderr.contains("Fresh probe")
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
fn a_commit_on_a_branch_rebuilds() {
    let root = create_probe("branch", &[]);
    let first = commit(&root);
    assert_eq!(built(&root), first);
    let second = commit(&root);
    assert_eq!(built(&root), second);
}

/// Git before 2.31, as Debian 11 ships it, prints `--path-format=absolute` back as it
/// is, in place of a path.
#[cfg(unix)]
#[test]
fn a_git_before_the_path_format_gives_the_commit() {
    let root = create_probe("old", &[]);
    let head = commit(&root);
    let path = create_shim(
        &root,
        "for a; do shift\n  if [ \"$a\" = --path-format=absolute ]; then echo \"$a\"; \
         else set -- \"$@\" \"$a\"; fi\ndone\nexec \"$real\" \"$@\"\n",
    );
    assert_eq!(built_with(&root, &[("PATH", &path)]), head);
}

/// Makes a `git` in `root` that runs the shell `body` with the real `git` in `$real`,
/// and gives the `PATH` of a build that runs it.
#[cfg(unix)]
fn create_shim(root: &Path, body: &str) -> String {
    let real = run(root, "sh", &["-c", "command -v git"], &[]);
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let shim = bin.join("git");
    std::fs::write(&shim, format!("#!/bin/sh\nreal={real}\n{body}")).expect("write");
    let mut permissions = std::fs::metadata(&shim).expect("stat").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&shim, permissions).expect("chmod");
    let toolchain = Path::new(env!("CARGO")).parent().expect("bin");
    format!("{}:{}:/usr/bin:/bin", bin.display(), toolchain.display())
}

/// Commits the probe at `root`, and adds a worktree of it on a new branch `other`.
fn create_worktree(root: &Path) -> PathBuf {
    git(root, &["add", "-A"]);
    commit(root);
    let path = root.with_extension("worktree");
    match std::fs::remove_dir_all(&path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            panic!("remove {}: {error}", path.display())
        }
        _ => {}
    }
    let worktree = path.to_str().expect("utf8");
    git(root, &["worktree", "add", "-q", "-b", "other", worktree]);
    path
}

/// Whether git can make a reftable repository, which needs git 2.45. Prints the skip
/// when it cannot: the runners of CI have git 2.43 until #2633.
#[expect(clippy::print_stdout, reason = "the test output names the skip")]
fn reftable() -> bool {
    let version = run(Path::new("."), "git", &["--version"], &[]);
    let number = version.trim_start_matches("git version ");
    let mut parts = number.split('.').map(|part| part.parse::<u32>().ok());
    let (Some(Some(major)), Some(Some(minor))) = (parts.next(), parts.next()) else {
        panic!("git gives the version `{version}`");
    };
    let able = (major, minor) >= (2, 45);
    if !able {
        println!("skips: git {number} cannot make a reftable repository");
    }
    able
}

#[test]
fn a_commit_and_a_switch_in_a_worktree_rebuild() {
    for (name, init) in [
        ("worktree", &[][..]),
        ("reftable-worktree", &["--ref-format=reftable"]),
    ] {
        if !init.is_empty() && !reftable() {
            continue;
        }
        let worktree = create_worktree(&create_probe(name, init));
        let first = git(&worktree, &["rev-parse", "HEAD"]);
        assert_eq!(built(&worktree), first, "{name}");
        assert!(fresh(&worktree), "{name}");
        let second = commit(&worktree);
        assert_eq!(built(&worktree), second, "{name}");
        git(&worktree, &["switch", "-q", "-c", "next", &first]);
        assert_eq!(built(&worktree), first, "{name}");
        git(&worktree, &["switch", "-q", "other"]);
        assert_eq!(built(&worktree), second, "{name}");
    }
}

/// Git refuses each other move of a branch that a worktree has checked out.
#[test]
fn an_update_of_the_ref_of_a_worktree_from_another_rebuilds() {
    for (name, init) in [
        ("moved", &[][..]),
        ("reftable-moved", &["--ref-format=reftable"]),
    ] {
        if !init.is_empty() && !reftable() {
            continue;
        }
        let root = create_probe(name, init);
        let worktree = create_worktree(&root);
        let first = git(&worktree, &["rev-parse", "HEAD"]);
        assert_eq!(built(&worktree), first, "{name}");
        let second = commit(&root);
        git(&root, &["update-ref", "refs/heads/other", &second]);
        assert_eq!(built(&worktree), second, "{name}");
    }
}

/// The error output of a build of the probe at `root` with `env`. Panics when the build
/// passes.
fn refused(root: &Path, env: &[(&str, &str)]) -> String {
    let output = Command::new(env!("CARGO"))
        .args(["build", "--offline", "--target-dir"])
        .arg(root.join("target"))
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .envs(env.iter().copied())
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!output.status.success(), "{stderr}");
    stderr
}

#[test]
fn a_broken_ref_stops_the_build() {
    let root = create_probe("broken", &[]);
    commit(&root);
    std::fs::write(root.join(".git/refs/heads/main"), "broken\n").expect("write");
    let stderr = refused(&root, &[]);
    assert!(
        stderr.contains("`git rev-parse HEAD` fails on a branch that has a commit"),
        "{stderr}"
    );
}

#[test]
fn a_commit_in_a_sha256_repository_gives_the_commit() {
    let root = create_probe("sha256", &["--object-format=sha256"]);
    let head = commit(&root);
    assert_eq!((head.len(), built(&root)), (64, head));
}

#[cfg(unix)]
#[test]
fn a_commit_that_is_not_lowercase_hex_stops_the_build() {
    let root = create_probe("upper", &[]);
    commit(&root);
    let upper = "0123456789ABCDEF0123456789ABCDEF01234567";
    let path = create_shim(
        &root,
        &format!(
            "if [ \"$1 $2\" = \"rev-parse HEAD\" ]; then echo {upper}; exit 0; fi\n\
             exec \"$real\" \"$@\"\n"
        ),
    );
    let stderr = refused(&root, &[("PATH", &path)]);
    let expected = format!(
        "`git rev-parse HEAD` gives `{upper}`, not 40 or 64 lowercase hex digits"
    );
    assert!(stderr.contains(&expected), "{stderr}");
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
    if !reftable() {
        return;
    }
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

#[test]
fn a_rewrite_of_packed_refs_while_the_ref_has_its_file_rebuilds_nothing() {
    let root = create_probe("rewrite", &[]);
    commit(&root);
    git(&root, &["branch", "other"]);
    git(&root, &["pack-refs", "--all"]);
    let head = commit(&root);
    assert_eq!(built(&root), head);
    assert!(fresh(&root));
    git(&root, &["branch", "-D", "other"]);
    assert!(fresh(&root));
    assert_eq!(built(&root), head);
}

#[test]
fn a_build_after_the_ref_moves_into_packed_refs_gives_the_commit() {
    let root = create_probe("pack", &[]);
    let head = commit(&root);
    assert_eq!(built(&root), head);
    git(&root, &["pack-refs", "--all"]);
    assert_eq!(built(&root), head);
    let next = commit(&root);
    assert_eq!(built(&root), next);
}
