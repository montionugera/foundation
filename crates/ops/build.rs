//! Sets `FOUNDATION_COMMIT` to the commit of `HEAD` when `git` reads one, and to the
//! empty string when it does not, so that no variable of the builder reaches `ops`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if git(&["rev-parse", "--git-dir"]).is_none() {
        println!("cargo::rustc-env=FOUNDATION_COMMIT=");
        return;
    }
    // Also on a branch with no commit, whose first commit must rebuild `ops`.
    watch_refs();
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    let hex = commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit());
    assert!(
        hex || commit.is_empty(),
        "`git rev-parse HEAD` gives `{commit}`, not 40 hex digits"
    );
    println!("cargo::rustc-env=FOUNDATION_COMMIT={commit}");
}

/// Watches what a commit, a checkout, and a branch switch change. In a worktree, `.git`
/// is a file, so only git knows where these are.
fn watch_refs() {
    if git(&["rev-parse", "--show-ref-format"]).as_deref() == Some("reftable") {
        // `HEAD` and each ref file are stubs: git writes each ref change here.
        watch(&git_path("reftable"));
        return;
    }
    watch(&git_path("HEAD"));
    if let Some(name) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch(&git_path(&name));
    }
    let packed = git_path("packed-refs");
    // `git pack-refs` makes it, and moves no ref to another commit.
    if packed.exists() {
        watch(&packed);
    }
}

/// The trimmed output of `git` with `args`, or `None` when it fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| text.trim().to_owned())
}

/// The absolute path of `name` in the git folder.
fn git_path(name: &str) -> PathBuf {
    let path = git(&["rev-parse", "--path-format=absolute", "--git-path", name]);
    PathBuf::from(path.unwrap_or_else(|| panic!("git gives no path of `{name}`")))
}

/// Watches `path`, or, when it is not there, the nearest folder above it that is: a
/// branch in `packed-refs` makes its loose ref file only at its next commit. Cargo
/// runs the script on each build while a watched path is missing.
fn watch(path: &Path) {
    let there = path
        .ancestors()
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("no folder above {} is there", path.display()));
    println!("cargo::rerun-if-changed={}", there.display());
}
