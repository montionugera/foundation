//! Checks of a merge queue group, read at the head of the PR of the group.

use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::{field, gh, review};

/// Runs the review check (`review::run`) for the PR that the queue ref `reference`
/// names, at the head of that PR: the second parent of the group commit `group`. The
/// repository at `root` must hold `group`. Returns the exit code of the review, or 2
/// when the ref names no PR or GitHub cannot be read.
pub(crate) fn review(root: &Path, reference: &str, group: &str) -> ExitCode {
    let target = pr(reference).and_then(|pr| Ok((pr, head_of(group)?)));
    match target {
        Ok((pr, head)) => {
            eprintln!("The group `{group}` holds PR {pr} at its head `{head}`.");
            review::run(root, &pr.to_string(), &head)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// Passes when the newest `mutants` check run at the head of the PR of the group
/// commit `group` ended in `success` or `skipped`. Fails with the reason otherwise.
pub(crate) fn mutants(group: &str) -> Result<(), Vec<String>> {
    let check = || {
        let head = head_of(group)?;
        let path = format!(
            "repos/{{owner}}/{{repo}}/commits/{head}/check-runs\
             ?check_name=mutants&filter=all"
        );
        let mut runs = Vec::new();
        for page in gh::api(&path)? {
            runs.extend_from_slice(field::list(&page, "check_runs")?);
        }
        verdict(&head, &runs)
    };
    let passed = check().map_err(|e| vec![e])?;
    eprintln!("{passed}");
    Ok(())
}

/// The number of the PR that the queue ref `reference` names, from its last part
/// `pr-<n>-<sha>`.
fn pr(reference: &str) -> Result<u64, String> {
    let last = reference
        .rsplit_once('/')
        .map_or(reference, |(_, last)| last);
    last.strip_prefix("pr-")
        .and_then(|rest| rest.split_once('-'))
        .filter(|(number, _)| number.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|(number, _)| number.parse().ok())
        .ok_or_else(|| {
            format!(
                "the queue ref `{reference}` names no PR: it does not end in \
                 `pr-<n>-<sha>`"
            )
        })
}

/// The head of the PR of the group commit `group`, read from GitHub.
fn head_of(group: &str) -> Result<String, String> {
    let commit = gh::api(&format!("repos/{{owner}}/{{repo}}/commits/{group}"))?;
    let commit = commit.first().ok_or("GitHub gave no commit")?;
    head(commit).map(str::to_string)
}

/// The head of the PR of a group commit: its second parent.
fn head(commit: &Value) -> Result<&str, String> {
    let parent = field::list(commit, "parents")?
        .get(1)
        .ok_or("the group commit has no second parent")?;
    field::text(parent, "sha")
}

/// Whether the newest of the `mutants` check `runs` at `head` passed: `Ok` with the
/// run, or `Err` with the reason.
fn verdict(head: &str, runs: &[Value]) -> Result<String, String> {
    let mut newest: Option<(u64, &Value)> = None;
    for run in runs {
        let id = field::number(run, "id")?;
        if newest.is_none_or(|(key, _)| id > key) {
            newest = Some((id, run));
        }
    }
    let (_, run) = newest
        .ok_or_else(|| format!("the head `{head}` has no `mutants` check run"))?;
    let url = field::text(run, "html_url")?;
    let named = format!("the newest `mutants` check run at the head `{head}` ({url})");
    let status = field::text(run, "status")?;
    if status != "completed" {
        return Err(format!("{named} has not ended: it is `{status}`"));
    }
    let conclusion = field::text(run, "conclusion")?;
    let ended = format!("{named} ended in `{conclusion}`");
    match conclusion {
        "success" | "skipped" => Ok(ended),
        _ => Err(ended),
    }
}

#[cfg(test)]
mod tests;
