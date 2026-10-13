//! Sets `FOUNDATION_COMMIT` to the commit of `HEAD` when `git` reads one.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let Some(commit) = git(&["rev-parse", "HEAD"]) else {
        return;
    };
    // A commit, a checkout, and a branch switch each change one of these. In a
    // worktree, `.git` is a file, so only git knows where they are.
    let mut names = vec!["HEAD".to_owned(), "packed-refs".to_owned()];
    names.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in &names {
        let path = git(&["rev-parse", "--path-format=absolute", "--git-path", name])
            .unwrap_or_else(|| panic!("git gives no path of `{name}`"));
        watch(Path::new(&path));
    }
    println!("cargo::rustc-env=FOUNDATION_COMMIT={commit}");
}

/// The trimmed output of `git` with `args`, or `None` when it fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| text.trim().to_owned())
}

/// Watches `path`, or, when it is not there, the nearest folder above it that is: a
/// branch in `packed-refs` makes its loose ref file only at its next commit. Cargo
/// runs the script on each build while a watched path is missing.
fn watch(path: &Path) {
    if path.ends_with("packed-refs") && !path.exists() {
        // `git pack-refs` makes it, and moves no ref to another commit.
        return;
    }
    let there = path
        .ancestors()
        .find(|path| path.exists())
        .unwrap_or_else(|| panic!("no folder above {} is there", path.display()));
    println!("cargo::rerun-if-changed={}", there.display());
}
