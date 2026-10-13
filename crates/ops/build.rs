//! Sets `FOUNDATION_COMMIT` to the commit of `HEAD` when `git` reads one, and to the
//! empty string when it does not, so that no variable of the builder reaches `ops`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Stops the build with the error of a checkout that gives no commit or a wrong one.
fn main() -> Result<(), String> {
    println!("cargo::rerun-if-changed=build.rs");
    if git(&["rev-parse", "--git-dir"]).is_none() {
        println!("cargo::rustc-env=FOUNDATION_COMMIT=");
        return Ok(());
    }
    // Also on a branch with no commit, whose first commit must rebuild `ops`.
    watch_refs()?;
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    if commit.is_empty() && !unborn() {
        return Err(
            "`git rev-parse HEAD` fails on a branch that has a commit".to_owned()
        );
    }
    let hex = matches!(commit.len(), 40 | 64)
        && commit
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !hex && !commit.is_empty() {
        return Err(format!(
            "`git rev-parse HEAD` gives `{commit}`, not 40 or 64 lowercase hex digits"
        ));
    }
    println!("cargo::rustc-env=FOUNDATION_COMMIT={commit}");
    Ok(())
}

/// Watches what a commit, a checkout, and a branch switch change. In a worktree, `.git`
/// is a file, so only git knows where these are.
fn watch_refs() -> Result<(), String> {
    if git(&["rev-parse", "--show-ref-format"]).as_deref() == Some("reftable") {
        // `HEAD` and each ref file are stubs. The stack of a worktree holds its `HEAD`,
        // and the common stack holds each branch.
        watch(&git_path("reftable")?);
        let common = git(&["rev-parse", "--git-common-dir"])
            .ok_or("git gives no common folder")?;
        watch(
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(common)
                .join("reftable"),
        );
        return Ok(());
    }
    watch(&git_path("HEAD")?);
    let Some(name) = git(&["symbolic-ref", "-q", "HEAD"]) else {
        return Ok(());
    };
    let path = git_path(&name)?;
    if path.exists() {
        // Git reads a ref file before `packed-refs`, which each worktree shares and
        // which a `git gc` in any of them rewrites.
        watch(&path);
        return Ok(());
    }
    let packed = git_path("packed-refs")?;
    if packed.exists() {
        watch(&packed);
    }
    watch_above(&path);
    Ok(())
}

/// Whether `HEAD` names a branch with no commit.
fn unborn() -> bool {
    let Some(name) = git(&["symbolic-ref", "-q", "HEAD"]) else {
        return false;
    };
    // `show-ref` exits with 1 for a missing ref, and with 128 for a broken one.
    let status = Command::new("git")
        .args(["show-ref", "--verify", "-q", &name])
        .status();
    status.is_ok_and(|status| status.code() == Some(1))
}

/// The trimmed output of `git` with `args`, or `None` when it fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| text.trim().to_owned())
}

/// The absolute path of `name` in the git folder. Git gives it relative to the folder
/// of the package, where cargo runs the script.
fn git_path(name: &str) -> Result<PathBuf, String> {
    let path = git(&["rev-parse", "--git-path", name]);
    let path = path.ok_or_else(|| format!("git gives no path of `{name}`"))?;
    Ok(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
}

/// Watches the nearest folder above `path` that is there, where git makes the file of a
/// ref at its next commit. Cargo runs the script on each build while a watched path is
/// missing.
fn watch_above(path: &Path) {
    let there = path.ancestors().find(|path| path.exists());
    watch(there.expect("invariant: the path is absolute, so `/` is there"));
}

/// Runs the script again when `path` changes.
fn watch(path: &Path) {
    println!("cargo::rerun-if-changed={}", path.display());
}
