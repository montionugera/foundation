use proptest::prelude::*;
use serde_json::json;

use super::*;
use crate::review::Repo;

/// The ref of the queue group of #2398.
const REF: &str = "refs/heads/gh-readonly-queue/main/\
                   pr-2398-d34e690883e5eae9bd83f0d16d357c69082d4338";

/// The head of #2398 when it merged.
const HEAD: &str = "652d62cd5";

fn run(id: u64, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "id": id,
        "status": status,
        "conclusion": conclusion,
        "html_url": format!("https://github.com/runs/{id}"),
    })
}

#[test]
fn reads_the_pr_of_a_queue_ref() {
    assert_eq!(pr(REF), Ok(2398));
    assert_eq!(pr(REF.trim_start_matches("refs/heads/")), Ok(2398));
}

#[test]
fn a_ref_with_no_pr_is_an_error() {
    for reference in [
        "refs/heads/main",
        "refs/heads/gh-readonly-queue/main/pr--d34e6908",
        "refs/heads/gh-readonly-queue/main/pr-x-d34e6908",
        "refs/heads/gh-readonly-queue/main/pr-2398",
        "refs/heads/gh-readonly-queue/main/pr-+2398-d34e6908",
        "refs/heads/gh-readonly-queue/pr-2398-d34e6908/main",
        "",
    ] {
        assert_eq!(
            pr(reference),
            Err(format!(
                "the queue ref `{reference}` names no PR: it does not end in \
                 `pr-<n>-<sha>`"
            ))
        );
    }
}

proptest! {
    #[test]
    fn reads_each_number_back(
        number: u64,
        base in "[a-z][a-z0-9._-]{0,8}(/[a-z0-9._-]{1,8}){0,2}",
        sha in "[0-9a-f]{40}",
    ) {
        let reference =
            format!("refs/heads/gh-readonly-queue/{base}/pr-{number}-{sha}");
        prop_assert_eq!(pr(&reference), Ok(number));
    }
}

/// A page of check runs, as GitHub gives it.
fn page(runs: &[Value]) -> Value {
    json!({ "total_count": runs.len(), "check_runs": runs })
}

/// A GitHub that holds the group commit `GROUP`, with the parents `parents`, and the
/// check run `pages` at `HEAD`.
fn github(
    parents: &[&str],
    pages: Vec<Value>,
) -> impl Fn(&str) -> Result<Vec<Value>, String> {
    let parents: Vec<Value> = parents.iter().map(|sha| json!({ "sha": sha })).collect();
    move |path| match path {
        "repos/{owner}/{repo}/commits/7a3039e97" => {
            Ok(vec![json!({ "parents": parents })])
        }
        "repos/{owner}/{repo}/commits/652d62cd5/check-runs?check_name=mutants&filter=all" => {
            Ok(pages.clone())
        }
        _ => Err(format!("gh api {path}: Not Found (HTTP 404)")),
    }
}

/// The group commit of #2398.
const GROUP: &str = "7a3039e97";

/// The first parent of `GROUP`.
const BASE: &str = "d34e69088";

#[test]
fn a_pr_number_past_u64_is_an_error() {
    let reference = "gh-readonly-queue/main/pr-18446744073709551616-d34e6908";
    assert_eq!(
        pr(reference),
        Err(format!(
            "the PR number `18446744073709551616` of the queue ref `{reference}` is not \
             a u64: number too large to fit in target type"
        ))
    );
}

#[test]
fn a_ref_that_names_no_pr_exits_2() {
    let offline = |path: &str| Err(format!("gh api {path}: offline"));
    assert_eq!(
        review(Path::new("."), "refs/heads/main", GROUP, &offline),
        ExitCode::from(2)
    );
}

#[test]
fn the_head_is_the_second_parent() {
    assert_eq!(
        head(GROUP, &github(&[BASE, HEAD], vec![])),
        Ok(HEAD.to_string())
    );
}

#[test]
fn a_group_commit_in_pages_has_the_parents_of_its_first() {
    let commit = json!({ "parents": [{ "sha": BASE }, { "sha": HEAD }] });
    let pages = |_: &str| Ok(vec![commit.clone(), commit.clone()]);
    assert_eq!(head(GROUP, &pages), Ok(HEAD.to_string()));
}

#[test]
fn a_group_commit_with_one_parent_is_an_error() {
    assert_eq!(
        head(GROUP, &github(&[BASE], vec![])),
        Err("the group commit `7a3039e97` has no second parent".to_string())
    );
}

#[test]
fn a_group_that_github_does_not_give_once_is_an_error() {
    assert_eq!(
        head(GROUP, &|_: &str| Ok(vec![])),
        Err("GitHub gave no commit `7a3039e97`".to_string())
    );
    assert_eq!(
        head("e1e1e1e1e", &github(&[BASE, HEAD], vec![])),
        Err(
            "gh api repos/{owner}/{repo}/commits/e1e1e1e1e: Not Found (HTTP 404)"
                .to_string()
        )
    );
}

#[test]
fn success_and_skipped_pass() {
    for conclusion in ["success", "skipped"] {
        assert_eq!(
            verdict(HEAD, &[page(&[run(5, "completed", Some(conclusion))])]),
            Ok(format!(
                "the newest `mutants` check run at the head `652d62cd5` \
                 (https://github.com/runs/5) ended in `{conclusion}`"
            ))
        );
    }
}

#[test]
fn each_other_conclusion_fails() {
    for conclusion in ["failure", "cancelled", "timed_out", "neutral"] {
        assert_eq!(
            verdict(HEAD, &[page(&[run(5, "completed", Some(conclusion))])]),
            Err(format!(
                "the newest `mutants` check run at the head `652d62cd5` \
                 (https://github.com/runs/5) ended in `{conclusion}`"
            ))
        );
    }
}

#[test]
fn a_run_that_has_not_ended_fails() {
    for status in ["queued", "in_progress"] {
        assert_eq!(
            verdict(HEAD, &[page(&[run(5, status, None)])]),
            Err(format!(
                "the newest `mutants` check run at the head `652d62cd5` \
                 (https://github.com/runs/5) has not ended: it is `{status}`"
            ))
        );
    }
}

#[test]
fn no_run_fails() {
    let failed = Err("the head `652d62cd5` has no `mutants` check run".to_string());
    assert_eq!(verdict(HEAD, &[]), failed);
    assert_eq!(verdict(HEAD, &[page(&[])]), failed);
}

#[test]
fn the_newest_run_of_each_page_decides() {
    let old = page(&[run(5, "completed", Some("success"))]);
    let new = page(&[
        run(7, "completed", Some("success")),
        run(9, "completed", Some("failure")),
    ]);
    let failed = Err("the newest `mutants` check run at the head `652d62cd5` \
         (https://github.com/runs/9) ended in `failure`"
        .to_string());
    assert_eq!(verdict(HEAD, &[old.clone(), new.clone()]), failed);
    assert_eq!(verdict(HEAD, &[new, old]), failed);
}

#[test]
fn a_run_with_no_id_is_an_error() {
    let run = json!({ "status": "completed", "conclusion": "success" });
    assert_eq!(
        verdict(HEAD, &[page(&[run])]),
        Err("JSON has no unsigned integer field `id`".to_string())
    );
}

#[test]
fn a_page_with_no_runs_is_an_error() {
    assert_eq!(
        verdict(HEAD, &[json!({ "message": "Not Found" })]),
        Err("JSON has no array field `check_runs`".to_string())
    );
}

#[test]
fn the_queue_reads_the_runs_at_the_head_of_the_pr() {
    let passed = page(&[run(5, "completed", Some("success"))]);
    let failed = page(&[run(9, "completed", Some("failure"))]);
    assert_eq!(
        mutants(GROUP, &github(&[BASE, HEAD], vec![passed.clone()])),
        Ok(())
    );
    assert_eq!(
        mutants(GROUP, &github(&[BASE, HEAD], vec![passed, failed])),
        Err(vec![
            "the newest `mutants` check run at the head `652d62cd5` \
             (https://github.com/runs/9) ended in `failure`"
                .to_string()
        ])
    );
    assert_eq!(
        mutants(GROUP, &github(&[HEAD, BASE], vec![])),
        Err(vec![
            "gh api repos/{owner}/{repo}/commits/d34e69088/check-runs\
             ?check_name=mutants&filter=all: Not Found (HTTP 404)"
                .to_string()
        ])
    );
}

/// A GitHub that holds the group commit `group` of `base` and `head`, and PR 2398
/// with a round comment that finds nothing and ends at `head`.
fn reviewed(
    base: &str,
    head: &str,
    group: &str,
) -> impl Fn(&str) -> Result<Vec<Value>, String> {
    let commit = json!({ "parents": [{ "sha": base }, { "sha": head }] });
    let pull = json!({
        "head": { "ref": "box1/2398-x" },
        "base": { "ref": "main" },
        "labels": [],
    });
    let round = format!(
        "**box1.red-team** · reviewer\nQuality: 8/10\nIt is good.\n\n\
         ## Review round 1\n\nReviewers: reviewer\nRange: `{base}..{head}`\n\
         Findings: none\n\nDeferred: none\nPublic surface: none\nHot path: none"
    );
    let comment = json!({
        "user": { "login": "synnax-foundation-factory[bot]" },
        "body": round,
        "created_at": "2026-10-12T00:00:00Z",
    });
    let group = format!("repos/{{owner}}/{{repo}}/commits/{group}");
    move |path| match path {
        _ if path == group => Ok(vec![commit.clone()]),
        "repos/{owner}/{repo}/pulls/2398" => Ok(vec![pull.clone()]),
        "repos/{owner}/{repo}/pulls/2398/files" => {
            Ok(vec![json!({ "filename": "b.txt" })])
        }
        "repos/{owner}/{repo}/issues/2398/comments" => Ok(vec![comment.clone()]),
        _ => Err(format!("gh api {path}: Not Found (HTTP 404)")),
    }
}

#[test]
fn the_queue_reviews_the_head_of_the_pr() {
    let (repo, head) = Repo::with_pr("queue-review");
    let base = repo.git(&["rev-parse", "main"]);
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    let group = repo.git(&["commit-tree", &tree, "-p", &base, "-p", &head, "-m", "g"]);
    let github = reviewed(&base, &head, &group);
    assert_eq!(review(&repo.dir, REF, &group, &github), ExitCode::SUCCESS);
    let found = reviewed(&base, &head, &group);
    let found = |path: &str| {
        found(path).map(|mut objects| {
            if path.ends_with("/comments") {
                objects[0]["body"] = objects[0]["body"]
                    .as_str()
                    .unwrap()
                    .replace("Findings: none", "Findings: 1")
                    .into();
            }
            objects
        })
    };
    assert_eq!(review(&repo.dir, REF, &group, &found), ExitCode::FAILURE);
    repo.remove();
}

#[test]
fn a_pr_that_github_does_not_give_exits_2() {
    assert_eq!(
        review(Path::new("."), REF, GROUP, &github(&[BASE, HEAD], vec![])),
        ExitCode::from(2)
    );
}
