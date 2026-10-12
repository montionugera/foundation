//! Rule 1 of `CLAUDE.md`: no commit and no PR body has a Claude attribution line.

use std::path::Path;
use std::process::Command;

/// Each line of `text` that attributes it to Claude, trimmed: a line that starts with
/// `Co-Authored-By:` and names Claude or `noreply@anthropic.com`, or a line with
/// "Generated with Claude Code" or "Generated with [Claude Code]", in any case.
pub(crate) fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|line| {
        let line = line.to_lowercase();
        let named = line.contains("claude") || line.contains("noreply@anthropic.com");
        (line.starts_with("co-authored-by:") && named)
            || line.contains("generated with claude code")
            || line.contains("generated with [claude code]")
    })
}

/// Checks the message of each commit that a push from the repository at `root` adds:
/// each commit of `HEAD` on no remote branch, merges too. Gives one problem for each
/// line ([`lines`]), with its commit.
pub(crate) fn check(root: &Path) -> Result<(), Vec<String>> {
    let problems = problems(root).map_err(|e| vec![e])?;
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn problems(root: &Path) -> Result<Vec<String>, String> {
    let args = ["log", "-z", "--format=%H%n%B", "HEAD", "--not", "--remotes"];
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .map_err(|e| format!("git log: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git log: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let log = String::from_utf8_lossy(&output.stdout);
    let mut problems = Vec::new();
    for commit in log.split_terminator('\0') {
        let (sha, message) = commit.split_once('\n').unwrap_or((commit, ""));
        problems.extend(lines(message).map(|line| {
            format!(
                "commit {sha} has the line `{line}`, which Rule 1 of CLAUDE.md \
                 forbids. Remove it with `git commit --amend` or `git rebase`, then \
                 push."
            )
        }));
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::Repo;

    const CLAUDE: &str = "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>";

    #[test]
    fn lines_finds_each_attribution_form_in_any_case() {
        let text = format!(
            "Fix a bug\n\n{CLAUDE}\nco-authored-by: claude\n\
             Co-Authored-By: Opus <NOREPLY@anthropic.com>\n  \
             🤖 Generated with [Claude Code](https://claude.com/claude-code)\n\
             generated with claude code\n"
        );
        assert_eq!(
            lines(&text).collect::<Vec<_>>(),
            [
                CLAUDE,
                "co-authored-by: claude",
                "Co-Authored-By: Opus <NOREPLY@anthropic.com>",
                "🤖 Generated with [Claude Code](https://claude.com/claude-code)",
                "generated with claude code",
            ]
        );
    }

    #[test]
    fn lines_passes_a_person_and_a_mention_of_claude() {
        let text = "Co-Authored-By: A Person <a@example.com>\n\
                    Reviewed-By: Claude\n\
                    The Claude Code harness adds a co-author line.\n";
        assert_eq!(lines(text).count(), 0);
    }

    fn commit(repo: &Repo, message: &str) -> String {
        repo.git(&["commit", "--quiet", "--allow-empty", "-m", message]);
        repo.git(&["rev-parse", "HEAD"])
    }

    fn problem(sha: &str, line: &str) -> String {
        format!(
            "commit {sha} has the line `{line}`, which Rule 1 of CLAUDE.md forbids. \
             Remove it with `git commit --amend` or `git rebase`, then push."
        )
    }

    #[test]
    fn check_refuses_each_line_of_a_commit_on_no_remote() {
        let (repo, _) = Repo::with_pr("attribution-local");
        let claude = commit(&repo, &format!("Fix a bug\n\n{CLAUDE}"));
        let lower = commit(&repo, "Fix a test\n\nco-authored-by: claude");
        assert_eq!(
            check(&repo.dir).unwrap_err(),
            [
                problem(&lower, "co-authored-by: claude"),
                problem(&claude, CLAUDE)
            ]
        );
        repo.remove();
    }

    #[test]
    fn check_passes_a_person_and_a_commit_already_on_a_remote() {
        let (repo, _) = Repo::with_pr("attribution-remote");
        let person = "Co-Authored-By: A Person <a@example.com>";
        commit(&repo, &format!("Fix a bug\n\n{person}"));
        assert_eq!(check(&repo.dir), Ok(()));
        let pushed = commit(&repo, &format!("Fix a bug\n\n{CLAUDE}"));
        repo.git(&["update-ref", "refs/remotes/origin/pr", &pushed]);
        assert_eq!(check(&repo.dir), Ok(()));
        repo.remove();
    }

    #[test]
    fn check_refuses_a_merge_with_the_line() {
        let (repo, _) = Repo::with_pr("attribution-merge");
        repo.git(&["switch", "--quiet", "-c", "side", "main"]);
        commit(&repo, "Side");
        repo.git(&["switch", "--quiet", "pr"]);
        let message = format!("Merge side\n\n{CLAUDE}");
        repo.git(&["merge", "--quiet", "--no-ff", "-m", &message, "side"]);
        let merge = repo.git(&["rev-parse", "HEAD"]);
        assert_eq!(check(&repo.dir).unwrap_err(), [problem(&merge, CLAUDE)]);
        repo.remove();
    }
}
