//! The skips check. The lists of each OS in `ci.yaml` are the one record of what that
//! OS does not run (LINUX CI), so a test skips an OS by an attribute only in a case of
//! `docs/claude/testing.md`, "No `#[ignore]`", and its reason says so.

use std::path::Path;

use crate::{files, select};

/// The words of a `cfg` predicate that pick an OS.
const OS: [&str; 4] = ["target_os", "target_family", "unix", "windows"];

/// Checks that no `.rs` file under `crates/` or `bench/` of the workspace at `root`
/// ignores a test on an OS with a reason that does not start with `only `.
pub(crate) fn check(root: &Path) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    for dir in ["crates", "bench"] {
        for file in files::rust(&root.join(dir)).map_err(|e| vec![e])? {
            let text = std::fs::read_to_string(&file)
                .map_err(|e| vec![format!("{}: {e}", file.display())])?;
            let shown = file
                .strip_prefix(root)
                .unwrap_or(&file)
                .display()
                .to_string();
            problems.extend(refusals(&shown, &text));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// The refusals of `text`, the source of the file shown as `shown`.
fn refusals(shown: &str, text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    for (at, _) in text.match_indices("#[cfg_attr(") {
        let line = text[..at].matches('\n').count() + 1;
        if text[..at]
            .rsplit('\n')
            .next()
            .is_some_and(|before| before.contains("//"))
        {
            continue;
        }
        let body = &text[at + "#[cfg_attr(".len()..];
        let body = &body[..select::closing(body)];
        let Some((predicate, rest)) = split(body) else {
            continue;
        };
        let rest = rest.trim();
        let words: String = predicate.split('"').step_by(2).collect();
        if !rest.starts_with("ignore")
            || !OS.iter().any(|w| select::has_word(&words, w))
        {
            continue;
        }
        let reason = rest["ignore".len()..]
            .trim_start()
            .strip_prefix('=')
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('"'));
        if !reason.is_some_and(|r| r.starts_with("only ")) {
            problems.push(format!(
                "`{shown}:{line}` ignores a test on an OS outside the lists of LINUX CI. \
                 Fix: remove the `ignore` and add `--skip <path> # #<issue>` to the \
                 `tests` list of each OS in `ci.yaml` where the test fails, or, in a case \
                 of `docs/claude/testing.md`, \"No `#[ignore]`\", start its reason with \
                 `only <OS>`."
            ));
        }
    }
    problems
}

/// Splits `body` at its first comma outside a group: the predicate and the attributes.
fn split(body: &str) -> Option<(&str, &str)> {
    let mut depth = 0;
    for (at, c) in body.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => return Some((&body[..at], &body[at + 1..])),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(at: &str) -> String {
        format!(
            "`{at}` ignores a test on an OS outside the lists of LINUX CI. Fix: remove \
             the `ignore` and add `--skip <path> # #<issue>` to the `tests` list of each \
             OS in `ci.yaml` where the test fails, or, in a case of \
             `docs/claude/testing.md`, \"No `#[ignore]`\", start its reason with \
             `only <OS>`."
        )
    }

    #[test]
    fn reads_the_files_under_crates_and_bench() {
        assert_eq!(
            check(&crate::fixture()),
            Err(["crates/a/src/skips.rs:3", "bench/b/src/skips.rs:1"]
                .map(refused)
                .into())
        );
    }

    #[test]
    fn refuses_each_ignore_on_an_os_whose_reason_does_not_start_with_only() {
        let text = "\
#[cfg_attr(not(target_os = \"linux\"), ignore = \"needs GNU nm\")]
#[test]
fn a() {}
#[cfg_attr(windows, ignore)]
fn b() {}
#[cfg_attr(
    not(any(target_os = \"linux\", target_family = \"wasm\")),
    ignore = \"needs /proc\"
)]
fn c() {}
#[cfg_attr(unix, ignore=\"Only Unix\")]
fn d() {}
";
        assert_eq!(
            refusals("x.rs", text),
            ["x.rs:1", "x.rs:4", "x.rs:6", "x.rs:11"].map(refused)
        );
    }

    #[test]
    fn passes_an_ignore_whose_reason_gives_its_case_and_each_other_cfg_attr() {
        let text = "\
#[cfg_attr(not(target_os = \"linux\"), ignore = \"only Linux: seccomp\")]
#[cfg_attr(miri, ignore = \"Miri cannot spawn a process\")]
#[cfg_attr(windows, expect(unused))]
#[cfg_attr(feature = \"unix\", ignore = \"a feature\")]
// #[cfg_attr(windows, ignore)]
";
        assert_eq!(refusals("x.rs", text), Vec::<String>::new());
    }
}
