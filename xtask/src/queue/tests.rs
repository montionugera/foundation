use proptest::prelude::*;
use serde_json::json;

use super::*;

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

#[test]
fn the_head_is_the_second_parent() {
    let commit = json!({ "parents": [{ "sha": "57fcf2050" }, { "sha": HEAD }] });
    assert_eq!(head(&commit), Ok(HEAD));
}

#[test]
fn a_group_commit_with_one_parent_is_an_error() {
    let commit = json!({ "parents": [{ "sha": "57fcf2050" }] });
    assert_eq!(
        head(&commit),
        Err("the group commit has no second parent".to_string())
    );
}

#[test]
fn success_and_skipped_pass() {
    for conclusion in ["success", "skipped"] {
        assert_eq!(
            verdict(HEAD, &[run(5, "completed", Some(conclusion))]),
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
            verdict(HEAD, &[run(5, "completed", Some(conclusion))]),
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
            verdict(HEAD, &[run(5, status, None)]),
            Err(format!(
                "the newest `mutants` check run at the head `652d62cd5` \
                 (https://github.com/runs/5) has not ended: it is `{status}`"
            ))
        );
    }
}

#[test]
fn no_run_fails() {
    assert_eq!(
        verdict(HEAD, &[]),
        Err("the head `652d62cd5` has no `mutants` check run".to_string())
    );
}

#[test]
fn the_newest_run_decides() {
    let old = run(5, "completed", Some("success"));
    let new = run(9, "completed", Some("failure"));
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
        verdict(HEAD, &[run]),
        Err("JSON has no unsigned integer field `id`".to_string())
    );
}
