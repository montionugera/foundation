//! Checks of a merge queue group, read at the head of the PR of the group.

use std::path::Path;
use std::process::ExitCode;

use serde_json::Value;

use crate::{field, review};

/// Reads the GitHub API: the objects that `gh api --paginate` gives for a path
/// (`gh::api`).
pub(crate) type Api<'a> = &'a dyn Fn(&str) -> Result<Vec<Value>, String>;

/// Runs the review check (`review::run`) for the PR that the queue ref `reference`
/// names, at the head of that PR: the second parent of the group commit `group`, read
/// with `api`. The repository at `root` must hold `group`. Returns the exit code of the
/// review, or 2 when the ref names no PR or GitHub cannot be read.
pub(crate) fn review(
    root: &Path,
    reference: &str,
    group: &str,
    api: Api<'_>,
) -> ExitCode {
    match pr(reference).and_then(|pr| Ok((pr, head(group, api)?))) {
        Ok((pr, head)) => {
            eprintln!("The group `{group}` holds PR {pr} at its head `{head}`.");
            review::run(root, &pr.to_string(), &head)
        }
        Err(e) => review::exit(Err(e)),
    }
}

/// Passes when the newest `mutants` check run at the head of the PR of the group
/// commit `group`, read with `api`, ended in `success` or `skipped`. Fails with the
/// reason otherwise.
pub(crate) fn mutants(group: &str, api: Api<'_>) -> Result<(), Vec<String>> {
    let check = || {
        let head = head(group, api)?;
        let pages = api(&format!(
            "repos/{{owner}}/{{repo}}/commits/{head}/check-runs\
             ?check_name=mutants&filter=all"
        ))?;
        verdict(&head, &pages)
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
    let number = last
        .strip_prefix("pr-")
        .and_then(|rest| rest.split_once('-'))
        .map(|(number, _)| number)
        .filter(|number| {
            !number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())
        })
        .ok_or_else(|| {
            format!(
                "the queue ref `{reference}` names no PR: it does not end in \
                 `pr-<n>-<sha>`"
            )
        })?;
    number.parse().map_err(|e| {
        format!(
            "the PR number `{number}` of the queue ref `{reference}` is not a u64: {e}"
        )
    })
}

/// The head of the PR of the group commit `group`, read with `api`: the second parent
/// of the commit.
fn head(group: &str, api: Api<'_>) -> Result<String, String> {
    let commits = api(&format!("repos/{{owner}}/{{repo}}/commits/{group}"))?;
    // A commit with more than 300 files comes in pages, each with the parents.
    let commit = commits
        .first()
        .ok_or_else(|| format!("GitHub gave no commit `{group}`"))?;
    let parent = field::list(commit, "parents")?
        .get(1)
        .ok_or_else(|| format!("the group commit `{group}` has no second parent"))?;
    field::text(parent, "sha").map(str::to_string)
}

/// Whether the newest `mutants` check run at `head`, of those in the check run `pages`
/// that GitHub gave, passed: `Ok` with the run, or `Err` with the reason.
fn verdict(head: &str, pages: &[Value]) -> Result<String, String> {
    let mut runs = Vec::new();
    for page in pages {
        for run in field::list(page, "check_runs")? {
            runs.push((field::number(run, "id")?, run));
        }
    }
    let (_, run) = runs
        .into_iter()
        .max_by_key(|(id, _)| *id)
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
