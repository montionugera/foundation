use super::*;

/// The head of #1089 when it merged.
const HEAD: &str = "c77c67d72fd8d37964334c80e14dd8b5a1d5b6fb";

/// The files that #1089 changed.
const FILES: &[&str] = &[
    "crates/config-hcl/src/arbitrary.rs",
    "crates/config-hcl/src/lib.rs",
    "crates/document/src/encoding.rs",
    "crates/spec/src/definition/tests.rs",
    "docs/decisions.md",
    "fuzz/fuzz_targets/config_hcl_read.rs",
    "oracles/conformance/document/encoding.rs",
];

/// The last round of #1089, replayed in the fixed format.
const ROUND: &str = "Quality: 8/10
The fixes close each round 1 finding. Tests pin the depth limit at both ends.

## Review round 3

Reviewers: reviewer
Breaker: skipped, the range changes no `.rs` line but comments
Range: `38cba24f..c77c67d7`
Findings: none

No bug, no lost coverage, no oracle weakening.

Deferred: none
Public surface: none
Hot path: none";

/// The end lines of `ROUND`.
const END: &str = "\n\nDeferred: none\nPublic surface: none\nHot path: none";

/// A comment by the bot, posted at the cutoff, so the end lines apply.
fn bot(body: &str) -> Comment {
    Comment {
        author: BOT.to_string(),
        body: body.to_string(),
        created: CUTOFF.to_string(),
    }
}

/// A comment by the bot, posted a second before the cutoff.
fn old(body: &str) -> Comment {
    Comment {
        created: "2026-10-08T02:59:59Z".to_string(),
        ..bot(body)
    }
}

fn record(comments: Vec<Comment>) -> Record {
    Record {
        branch: "laptop/828-checked-document".to_string(),
        base: "main".to_string(),
        labels: Vec::new(),
        files: FILES.iter().map(ToString::to_string).collect(),
        comments,
    }
}

/// A round 2 that names each reviewer a code PR requires, as the first round comment
/// of a PR that replaces another.
fn opening() -> Comment {
    bot(&ROUND
        .replace("## Review round 3", "## Review round 2")
        .replace(
            "Reviewers: reviewer",
            "Reviewers: reviewer, architecture, breaker",
        )
        .replace(
            "Breaker: skipped, the range changes no `.rs` line but comments\n",
            "",
        ))
}

/// A record of a PR with `opening` before `comments`.
fn reviewed(mut comments: Vec<Comment>) -> Record {
    comments.insert(0, opening());
    record(comments)
}

/// The start of the range of `ROUND`, which changes only comment lines.
const COMMENTS: &str = "38cba24f";

/// A commit on the first-parent chain of `HEAD` that only clean merges of the base
/// follow.
const MERGED: &str = "3d9a61c0";

/// A commit on the first-parent chain of `HEAD` from which the range to `HEAD`
/// changes no code.
const TEXT: &str = "5e0f17a2";

/// A commit on the first-parent chain of `HEAD` from which the range to `HEAD`
/// changes code.
const CODE: &str = "82ba5b72";

fn check(record: &Record) -> Vec<String> {
    let head = |commit: &str| HEAD.starts_with(commit);
    problems(
        record,
        HEAD,
        &|end| {
            Ok(match end {
                _ if head(end) => Place::Head,
                MERGED => Place::Head,
                TEXT | CODE => Place::Before,
                _ => Place::Off,
            })
        },
        &|from, _| {
            let text = head(from) || [COMMENTS, MERGED, TEXT].contains(&from);
            Ok((!text).then(|| "changes code at `a.rs:2`".to_string()))
        },
        &|_, _| Ok(None),
    )
    .unwrap()
}

#[test]
fn passes_the_last_round_of_1089() {
    let earlier = bot(
        &("## Review round 2\n\nReviewers: reviewer, breaker\nRange: `a..b`\n\
           Findings: 2"
            .to_string()
            + END),
    );
    assert_eq!(
        check(&reviewed(vec![earlier, bot(ROUND)])),
        Vec::<String>::new()
    );
}

#[test]
fn fails_each_free_form_earlier_round() {
    let free = |n| {
        bot(&format!(
            "## Review round {n}\n\nConfirmed findings, most severe first.\n\n\
             1. **`Checked::new` overflows the stack**."
        ))
    };
    let missing = |n| {
        format!(
            "review round {n} has no `Range:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
        )
    };
    assert_eq!(
        check(&record(vec![free(1), free(2), bot(ROUND)])),
        vec![missing(1), missing(2)]
    );
}

/// Round 1 of a code PR with each reviewer it requires, its findings, and `end`.
fn first(end: &str) -> Comment {
    bot(&format!(
        "## Review round 1\n\nReviewers: reviewer, architecture, breaker\n\
         Range: `a..b`\nFindings: 2\n\n1. **`send` drops a frame**.{end}"
    ))
}

/// `first(END)` with no findings.
fn settled() -> Comment {
    let round = first(END);
    Comment {
        body: round.body.replace("Findings: 2", "Findings: none"),
        ..round
    }
}

#[test]
fn fails_an_earlier_round_with_no_end_line() {
    assert_eq!(
        check(&record(vec![first(""), bot(ROUND)])),
        vec![unended("Deferred").replace("round 3", "round 1")]
    );
}

#[test]
fn an_earlier_round_that_names_a_hot_path_needs_performance() {
    let hot = END.replace("Hot path: none", "Hot path: `stream::Sender::send`");
    assert_eq!(
        check(&record(vec![first(&hot), bot(ROUND)])),
        vec![
            "review round 1 names no performance, which this round requires."
                .to_string()
        ]
    );
}

/// The problem of a round 3 with raw HTML in `line`.
fn raw(line: &str) -> String {
    format!(
        "review round 3 has raw HTML, which can hide text on GitHub, in the line \
         `{line}`. Put the line in a code span, in the format of \
         .claude/skills/review/SKILL.md, \"Round comment\"."
    )
}

/// The problem of a round 3 with `[^` before the last line of a paragraph in `line`.
fn bracket(line: &str) -> String {
    format!(
        "review round 3 has `[^` before the last line of a paragraph, which can hide \
         text on GitHub, in the line `{line}`. Put the line in a code span, in the \
         format of .claude/skills/review/SKILL.md, \"Round comment\"."
    )
}

/// The problem of a round 3 with a paragraph that comrak places in the wrong lines,
/// which starts with `line`.
fn misplaced(line: &str) -> String {
    format!(
        "review round 3 has a link or an image with a line break after its text, or a \
         link reference definition, in the paragraph that starts with the line \
         `{line}`, which the check cannot read. Write each link and image on one line, \
         and put a blank line after each link reference definition, in the format of \
         .claude/skills/review/SKILL.md, \"Round comment\"."
    )
}

/// The problem of a round 3 that does not end with its `name:` line.
fn unended(name: &str) -> String {
    format!(
        "review round 3 does not end with a `{name}:` line. End each round with its \
         `Deferred:`, `Public surface:`, and `Hot path:` lines, in that order, in the \
         format of .claude/skills/review/SKILL.md, \"Round comment\"."
    )
}

#[test]
fn fails_a_round_with_no_end_line() {
    let cases = [
        ("Deferred: none\n", unended("Deferred")),
        ("Public surface: none\n", unended("Public surface")),
        ("\nHot path: none", unended("Hot path")),
    ];
    for (line, problem) in cases {
        let round = ROUND.replace(line, "");
        assert_eq!(check(&record(vec![bot(&round)])), vec![problem], "{line}");
    }
}

#[test]
fn fails_a_round_whose_end_lines_are_out_of_order_or_not_last() {
    let swapped = ROUND.replace(
        "Deferred: none\nPublic surface: none",
        "Public surface: none\nDeferred: none",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&swapped)])),
        vec![unended("Deferred")]
    );
    let twice = ROUND.to_string() + "\nHot path: `Sender::send`";
    assert_eq!(
        check(&reviewed(vec![bot(&twice)])),
        vec![
            "review round 3 has a second `Hot path:` line in its end lines."
                .to_string()
        ]
    );
    let spaced = ROUND.replace("\nHot path:", "\n\nHot path:");
    assert_eq!(
        check(&reviewed(vec![bot(&spaced)])),
        vec![unended("Deferred")]
    );
    let indented = ROUND.replace("\nHot path:", "\n  Hot path:");
    assert_ne!(indented, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&indented)])), Vec::<String>::new());
    let trailed = ROUND.replace("none\n", "none  \n") + "\n  \n";
    assert_eq!(check(&reviewed(vec![bot(&trailed)])), Vec::<String>::new());
    let followed = ROUND.to_string() + "\n\nThe author fixes each finding.";
    assert_eq!(
        check(&reviewed(vec![bot(&followed)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn an_old_round_keeps_the_check_before_the_end_lines() {
    let free = old("## Review round 1\n\nConfirmed findings, most severe first.");
    let unended = old(&ROUND.replace(END, ""));
    assert_eq!(check(&record(vec![free, unended])), Vec::<String>::new());
    let missing = |n, name| {
        format!(
            "review round {n} has no `{name}:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
        )
    };
    for (fields, name) in [
        ("Reviewers: reviewer", "Range"),
        ("Findings: none", "Range"),
    ] {
        let fixed = old(&format!("## Review round 1\n\n{fields}"));
        assert_eq!(
            check(&record(vec![fixed, bot(ROUND)])),
            vec![missing(1, name)],
            "{fields}"
        );
    }
    let last = old("## Review round 4\n\nNo findings.");
    assert_eq!(check(&record(vec![last])), vec![missing(4, "Range")]);
}

#[test]
fn an_old_round_that_names_a_hot_path_needs_performance() {
    let problem = vec![
        "review round 3 names no performance, which this round requires.".to_string(),
    ];
    let cases = [
        ("Hot path: none", "Hot path: `send`"),
        ("Findings: none", "Findings: none\nHot path: `send`"),
        ("Hot path: none", "Hot path:\n- `send`, once per frame"),
        (
            "\n\nDeferred: none\nPublic surface: none\n",
            "\nHot path: `send`\n",
        ),
    ];
    for (line, hot) in cases {
        let round = old(&ROUND.replace(line, hot));
        assert_eq!(check(&reviewed(vec![round])), problem, "{hot}");
    }
    let quoted = ROUND.replace(
        "Hot path: none",
        "Hot path: none\n\n```\nHot path: `send`\n```",
    );
    let tildes = ROUND.replace(
        "Hot path: none",
        "Hot path: none\n\n~~~\nHot path: `send`\n~~~",
    );
    let indented =
        ROUND.replace("Hot path: none", "Hot path: none\n\n    Hot path: `send`");
    let wrapped = ROUND.replace("Hot path: none", "Hot path:\nnone, tests only");
    for round in [quoted, tildes, indented, wrapped] {
        assert_eq!(
            check(&reviewed(vec![old(&round)])),
            Vec::<String>::new(),
            "{round}"
        );
    }
}

#[test]
fn fails_a_round_whose_end_lines_are_only_quoted_in_its_text() {
    let quoted = ROUND.replace(
        "No bug, no lost coverage, no oracle weakening.\n\n\
         Deferred: none\nPublic surface: none\nHot path: none",
        "The breaker tried a round that ends with\n\n```\nDeferred: none\n\
         Public surface: none\nHot path: none\n```\n\nand one that names \
         `stream::Sender::send` as its hot path. None fails.",
    );
    assert_ne!(quoted, ROUND);
    assert_eq!(
        check(&record(vec![bot(&quoted)])),
        vec![unended("Deferred")]
    );
    let indented = ROUND.replace(
        "No bug, no lost coverage, no oracle weakening.\n\n\
         Deferred: none\nPublic surface: none\nHot path: none",
        "The breaker tried a round that ends with these lines:\n\n    \
         Deferred: none\n    Public surface: none\n    Hot path: none",
    );
    assert_ne!(indented, ROUND);
    assert_eq!(
        check(&record(vec![bot(&indented)])),
        vec![unended("Deferred")]
    );
    let fenced = ROUND.replace(
        "No bug, no lost coverage, no oracle weakening.\n\n\
         Deferred: none\nPublic surface: none\nHot path: none",
        "The breaker posted this round:\n\n```\nFindings: none\n\n\
         Deferred: none\nPublic surface: none\nHot path: none\n```",
    );
    assert_ne!(fenced, ROUND);
    assert_eq!(
        check(&record(vec![bot(&fenced)])),
        vec![unended("Deferred")]
    );
    let closed = ROUND.to_string() + "\n```\nHot path: `send`\n```";
    assert_eq!(
        check(&record(vec![bot(&closed)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn fails_end_lines_in_a_code_block_that_is_not_closed() {
    for fence in ["```", "~~~", "````"] {
        let unclosed =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{fence}\n\n"));
        assert_ne!(unclosed, ROUND);
        assert_eq!(
            check(&record(vec![bot(&unclosed)])),
            vec![unended("Deferred")],
            "{fence}"
        );
    }
    let indented = ROUND.replace(
        "weakening.\n\nDeferred: none\nPublic surface: none\nHot path: none",
        "weakening.\n\n   ```\n\nDeferred: none\nPublic surface: none\n\
         Hot path: none\n   ```",
    );
    assert_ne!(indented, ROUND);
    assert_eq!(
        check(&record(vec![bot(&indented)])),
        vec![unended("Deferred")]
    );
    for indent in ["    ", "\t", "\u{a0}"] {
        let unclosed = ROUND.replace(
            "weakening.\n\n",
            &format!("weakening.\n\n```\n{indent}```\n\n"),
        );
        assert_eq!(
            check(&record(vec![bot(&unclosed)])),
            vec![unended("Deferred")],
            "{indent:?}"
        );
    }
    let short = ROUND.replace("weakening.\n\n", "weakening.\n\n````\n```\n\n");
    assert_eq!(check(&record(vec![bot(&short)])), vec![unended("Deferred")]);
    let other = ROUND.replace("weakening.\n\n", "weakening.\n\n```\n~~~\n\n");
    assert_eq!(check(&record(vec![bot(&other)])), vec![unended("Deferred")]);
    let info = ROUND.replace("weakening.\n\n", "weakening.\n\n```\n``` rust\n\n");
    assert_eq!(check(&record(vec![bot(&info)])), vec![unended("Deferred")]);
}

#[test]
fn reads_a_fence_with_four_spaces_as_text() {
    let shown = later("reviewer, breaker").replace(
        "Hot path: none",
        "Hot path: `send`\n\n```\n\n    ```\n\nDeferred: none\nPublic surface: none\n\
         Hot path: none",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&shown)])),
        vec![unended("Deferred")]
    );
    let quoted = ROUND.replace(
        "weakening.\n\nDeferred",
        "weakening.\n\n```\n\n    ```\n\nDeferred",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&quoted)])),
        vec![unended("Deferred")]
    );
    let hidden = ROUND.replace(
        "Hot path: none",
        "Hot path: none\n\n    ```\n\nHot path: `send`",
    );
    assert_eq!(
        check(&reviewed(vec![old(&hidden)])),
        vec![
            "review round 3 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn reads_a_tilde_fence_by_its_length_and_any_info() {
    let struck = ROUND.replace("No bug,", "~~a~~ b. No bug,");
    assert_ne!(struck, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&struck)])), Vec::<String>::new());
    let quoted = ROUND.replace(
        "weakening.\n\nDeferred",
        "weakening.\n\n~~~ `a`\n\nDeferred",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&quoted)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn an_old_free_form_earlier_round_that_names_a_hot_path_needs_performance() {
    let free = old("## Review round 1\n\nConfirmed findings.\n\nHot path: `send`");
    assert_eq!(
        check(&record(vec![free, bot(ROUND)])),
        vec![
            "review round 1 names no performance, which this round requires."
                .to_string()
        ]
    );
    let quiet = old("## Review round 1\n\nConfirmed findings.\n\nHot path: none");
    assert_eq!(
        check(&record(vec![quiet, bot(ROUND)])),
        Vec::<String>::new()
    );
    let unnumbered = old("## Review round one\n\nHot path: `send`");
    assert_eq!(
        check(&record(vec![unnumbered, bot(ROUND)])),
        vec![
            "review round one names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn reads_an_indented_heading_and_indented_fields() {
    let indented = ROUND
        .replace("## Review", "  ## Review")
        .replace("\nReviewers:", "\n  Reviewers:")
        .replace("\nRange:", "\n  Range:")
        .replace("\nFindings:", "\n  Findings:");
    assert_ne!(indented, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&indented)])), Vec::<String>::new());
    assert_eq!(check(&reviewed(vec![old(&indented)])), Vec::<String>::new());
}

#[test]
fn reads_inline_code_at_the_start_of_a_line_as_text() {
    let inline = ROUND.replace("No bug,", "```cargo xtask review``` passes. No bug,");
    assert_ne!(inline, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&inline)])), Vec::<String>::new());
    for indent in ["    ", "\t", "\u{a0}"] {
        let text =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{indent}```\n\n"));
        assert_eq!(
            check(&reviewed(vec![bot(&text)])),
            Vec::<String>::new(),
            "{indent:?}"
        );
    }
    let fenced = ROUND.replace(
        "weakening.\n\n",
        "weakening.\n\n``` rust\nlet a = 1;\n```\n\n",
    );
    assert_ne!(fenced, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&fenced)])), Vec::<String>::new());
}

#[test]
fn fails_end_lines_that_are_not_a_paragraph_of_their_own() {
    let joined = ROUND.replace("weakening.\n\n", "weakening.\n");
    assert_ne!(joined, ROUND);
    assert_eq!(
        check(&record(vec![bot(&joined)])),
        vec![unended("Deferred")]
    );
    let alone = "## Review round 3\n\nDeferred: none\nPublic surface: none\n\
                 Hot path: none\nReviewers: reviewer\nRange: `38cba24f..c77c67d7`\n\
                 Findings: none";
    assert_eq!(check(&record(vec![bot(alone)])), vec![unended("Deferred")]);
}

#[test]
fn reads_an_end_line_that_wraps() {
    let wrapped = ROUND
        .replace(
            "Deferred: none",
            "Deferred: the trial, to #1467\n(https://github.com/a/b/issues/1467)",
        )
        .replace(
            "Hot path: none",
            "Hot path: none, the change\nis in tests only",
        );
    assert_eq!(check(&reviewed(vec![bot(&wrapped)])), Vec::<String>::new());
    let next = later("reviewer, breaker").replace(
        "Hot path: none",
        "Hot path:\nnone, the change is in tests only",
    );
    assert_eq!(check(&reviewed(vec![bot(&next)])), Vec::<String>::new());
    let block = ROUND.replace("weakening.", "weakening:\n\n```\nHot path: `send`\n```");
    assert_eq!(check(&reviewed(vec![bot(&block)])), Vec::<String>::new());
    let earlier = ROUND.replace(
        "No bug",
        "Deferred: none\nPublic surface: none\nHot path: `send`\n\nNo bug",
    );
    assert_eq!(check(&reviewed(vec![bot(&earlier)])), Vec::<String>::new());
}

#[test]
fn a_round_that_names_a_hot_path_needs_performance() {
    let hot = later("reviewer, breaker")
        .replace("Hot path: none", "Hot path: stream::Sender::send");
    assert_eq!(
        check(&reviewed(vec![bot(&hot)])),
        vec![
            "review round 3 names no performance, which this round requires."
                .to_string()
        ]
    );
    let prefixed = later("reviewer, breaker").replace(
        "Hot path: none",
        "Hot path: nonempty_frames, once per frame",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&prefixed)])),
        check(&reviewed(vec![bot(&hot)]))
    );
    let listed = later("reviewer, breaker").replace(
        "Hot path: none",
        "Hot path:\n- `stream::Sender::send`, once per frame",
    );
    assert_eq!(
        check(&reviewed(vec![bot(&listed)])),
        vec![unended("Deferred")]
    );
    let called = later("reviewer, breaker")
        .replace("Hot path: none", "Hot path: `none()`, once per frame");
    assert_eq!(
        check(&reviewed(vec![bot(&called)])),
        check(&reviewed(vec![bot(&hot)]))
    );
    for value in [",none", "none..", "none;,", "`none`.."] {
        let punctuated = later("reviewer, breaker")
            .replace("Hot path: none", &format!("Hot path: {value}"));
        assert_eq!(
            check(&reviewed(vec![bot(&punctuated)])),
            check(&reviewed(vec![bot(&hot)])),
            "{value}"
        );
    }
    for value in ["none.", "none;", "`none`.", "`none`;"] {
        let quiet = later("reviewer, breaker")
            .replace("Hot path: none", &format!("Hot path: {value}"));
        assert_eq!(
            check(&reviewed(vec![bot(&quiet)])),
            Vec::<String>::new(),
            "{value}"
        );
    }
    let quiet = later("reviewer, breaker")
        .replace("Hot path: none", "Hot path: `none`, tests only");
    assert_eq!(check(&reviewed(vec![bot(&quiet)])), Vec::<String>::new());
    let measured = hot.replace("breaker\n", "breaker, performance\n");
    assert_eq!(check(&reviewed(vec![bot(&measured)])), Vec::<String>::new());
    let cold = later("reviewer, breaker").replace(
        "Hot path: none",
        "Hot path: none, `send` runs once per stream",
    );
    assert_eq!(check(&reviewed(vec![bot(&cold)])), Vec::<String>::new());
    let docs = ROUND.replace("Hot path: none", "Hot path: `Sender::send`");
    let mut record = reviewed(vec![bot(&docs)]);
    record.files = vec!["docs/decisions/crate-map.md".to_string()];
    assert_eq!(
        check(&record),
        vec![
            "review round 3 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn fails_an_earlier_round_with_fields_and_no_number() {
    // GitHub does not show the raw HTML `<n>`.
    for (heading, shown) in [
        ("## Review round 1.", "## Review round 1."),
        ("## Review round <n>", "## Review round "),
    ] {
        let first = bot(&format!("{heading}\n\nReviewers: reviewer"));
        assert_eq!(
            check(&record(vec![first, bot(ROUND)])),
            vec![format!("`{shown}` has no round number")]
        );
    }
    // The number comes before raw HTML.
    let html = bot("## Review round x\n\nReviewers: reviewer\n\n<div>");
    assert_eq!(
        check(&record(vec![html, bot(ROUND)])),
        vec!["`## Review round x` has no round number"]
    );
}

#[test]
fn reads_each_name_as_github_shows_it() {
    // GitHub shows each line with `Hot path:` at its start.
    for line in [
        "&#72;ot path: `send`",
        "**Hot path:** `send`",
        "`Hot path:` `send`",
        "~~Hot path:~~ `send`",
        "` `Hot path: `send`",
        "&#32;Hot path: `send`",
        "&#9;Hot path: `send`",
        "&#10;Hot path: `send`",
        "&nbsp;Hot path: `send`",
        "&#8203;Hot path: `send`",
        "&ZeroWidthSpace;Hot path: `send`",
        "&shy;Hot path: `send`",
        "&#xFEFF;Hot path: `send`",
        "Hot&nbsp;path: `send`",
        "Hot&#8203; path: `send`",
        "Hot&#x2003;&#x2003;path: `send`",
    ] {
        let comment =
            ROUND.replace("Hot path: none", &format!("{line}\nHot path: none"));
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            vec!["review round 3 has a second `Hot path:` line in its end lines."],
            "{line}"
        );
    }
    for (plain, shown) in [
        ("Reviewers: reviewer", "Reviewers\\: reviewer"),
        ("Deferred: none", "Deferred\\: none"),
        ("Hot path: none", "Hot path\\: none"),
        // GitHub shows the image, not its text.
        ("Hot path: none", "![Hot path:](x) `send`\nHot path: none"),
        // A Cyrillic `о` in place of the Latin one.
        ("Hot path: none", "H\u{43e}t path: `send`\nHot path: none"),
    ] {
        let comment = ROUND.replace(plain, shown);
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            Vec::<String>::new(),
            "{shown}"
        );
    }
}

#[test]
fn reads_each_field_as_github_shows_it() {
    for line in [
        "Findings\\: 2",
        "&#32;Findings: 2",
        "Findings:\t2",
        "Findings:&#32;2",
        "Findings:&nbsp;2",
    ] {
        let comment =
            second().replace("Findings: none", &format!("{line}\nFindings: none"));
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            vec![
                "review round 2 has findings (2). Fix or answer them, then run another \
                 round."
            ],
            "{line}"
        );
    }
    let range = second().replace(
        "Range: `38cba24f",
        "Range\\: `aaaaaaaa..bbbbbbbb`\nRange: `38cba24f",
    );
    assert_ne!(range, second());
    assert_eq!(
        check(&reviewed(vec![bot(&range)])),
        vec![
            "review round 2 skips `breaker`, but its range changes code at `a.rs:2`.",
            "review round 2 ends at bbbbbbbb, not at the head \
             c77c67d72fd8d37964334c80e14dd8b5a1d5b6fb. A commit after the round needs \
             a new round; only a clean merge of the base, or a commit that changes no \
             code after a round with a `Text fixes:` line, does not.",
        ]
    );
}

#[test]
fn reads_no_invisible_character() {
    // Both ends of each range of Unicode default ignorable code points.
    for c in [
        '\u{AD}',
        '\u{34F}',
        '\u{61C}',
        '\u{115F}',
        '\u{1160}',
        '\u{17B4}',
        '\u{17B5}',
        '\u{180B}',
        '\u{180F}',
        '\u{200B}',
        '\u{200F}',
        '\u{202A}',
        '\u{202E}',
        '\u{2060}',
        '\u{206F}',
        '\u{3164}',
        '\u{FE00}',
        '\u{FE0F}',
        '\u{FEFF}',
        '\u{FFA0}',
        '\u{FFF0}',
        '\u{FFF8}',
        '\u{1BCA0}',
        '\u{1BCA3}',
        '\u{1D173}',
        '\u{1D17A}',
        '\u{E0000}',
        '\u{E0FFF}',
    ] {
        let comment = ROUND.replace(
            "Hot path: none",
            &format!("{c}Hot path: `send`\nHot path: none"),
        );
        assert_eq!(
            check(&record(vec![bot(&comment)])),
            vec!["review round 3 has a second `Hot path:` line in its end lines."],
            "{:X}",
            u32::from(c)
        );
    }
}

#[test]
fn reads_off_the_backticks_that_github_shows() {
    let escaped = ROUND
        .replace("Reviewers: reviewer", "Reviewers: \\`reviewer\\`")
        .replace("`38cba24f..c77c67d7`", "\\`38cba24f..c77c67d7\\`");
    assert_eq!(check(&reviewed(vec![bot(&escaped)])), Vec::<String>::new());
    let code = ROUND.replace("Hot path: none", "Hot path: `Sender::send`");
    let shown = ROUND.replace("Hot path: none", "Hot path: \\`Sender::send\\`");
    let mut code = reviewed(vec![bot(&code)]);
    let mut shown = reviewed(vec![bot(&shown)]);
    code.files = vec!["docs/decisions/crate-map.md".to_string()];
    shown.files = code.files.clone();
    assert_eq!(
        check(&shown),
        vec!["review round 3 names no performance, which this round requires."]
    );
    assert_eq!(check(&shown), check(&code));
    let none = ROUND.replace("Hot path: none", "Hot path: \\`none\\`");
    let mut none = reviewed(vec![bot(&none)]);
    none.files = code.files;
    assert_eq!(check(&none), Vec::<String>::new());
}

#[test]
fn reads_the_round_heading_as_github_shows_it() {
    let fields = second().replace("Findings: none", "Findings: 2");
    for heading in [
        "## Review&#32;round 2",
        "## Review *round* 2",
        "## Review\u{200B} round 2",
        "Review round 2\n---",
    ] {
        let last = fields.replace("## Review round 2", heading);
        assert_eq!(
            check(&reviewed(vec![bot(&second()), bot(&last)])),
            vec![
                "review round 2 has findings (2). Fix or answer them, then run \
                 another round."
            ],
            "{heading}"
        );
    }
    let word = fields.replace("## Review round 2", "## Review roundup 2");
    assert_eq!(
        check(&reviewed(vec![bot(&second()), bot(&word)])),
        Vec::<String>::new()
    );
    // GitHub shows these headings in a quote, or of level 1: not a round.
    for heading in ["# Review round 2", "<search\n> ## Review round 2"] {
        let other = fields.replace("## Review round 2", heading);
        assert_eq!(
            check(&reviewed(vec![bot(&second()), bot(&other)])),
            Vec::<String>::new(),
            "{heading}"
        );
    }
    let lines = fields.replace("## Review round 2", "Review round\n3\n---");
    assert_eq!(
        check(&reviewed(vec![bot(&second()), bot(&lines)])),
        vec!["`## Review round ` has no round number"]
    );
}

#[test]
fn an_old_round_counts_a_name_where_github_shows_it() {
    let entity =
        old("## Review round 1\n\nConfirmed a finding.\n\n&#72;ot path: `send`");
    let escaped = old(
        "## Review round 1\n\nConfirmed a finding.\n\nReviewers\\: performance\n\n\
         Hot path: `send`",
    );
    for (round, problems) in [
        (
            entity,
            vec![
                "review round 1 names no performance, which this round requires."
                    .to_string(),
            ],
        ),
        (escaped, Vec::new()),
    ] {
        assert_eq!(check(&record(vec![round, bot(ROUND)])), problems);
    }
}

#[test]
fn fails_an_earlier_round_in_the_fixed_format_that_does_not_parse() {
    let missing = |name| {
        format!(
            "review round 1 has no `{name}:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
        )
    };
    let cases = [
        ("Reviewers: reviewer", missing("Range")),
        ("Range: `a..b`", missing("Findings")),
        ("Findings: none", missing("Range")),
        (
            "Reviewers: reviewer\nRange: `a..b`\nFindings: 2 (R1, B1)",
            "review round 1 has `Findings: 2 (R1, B1)`, not a count or `none`: \
             invalid digit found in string"
                .to_string(),
        ),
    ];
    for (fields, problem) in cases {
        let first = bot(&format!("## Review round 1\n\n{fields}"));
        assert_eq!(check(&record(vec![first, bot(ROUND)])), vec![problem]);
    }
}

#[test]
fn reads_the_range_of_the_last_round_only() {
    let skipped = ROUND
        .replace(COMMENTS, "940140aa")
        .replace("round 3", "round 2");
    let last = later("reviewer, breaker");
    assert_eq!(
        check(&reviewed(vec![bot(&skipped), bot(&last)])),
        Vec::<String>::new()
    );
}

#[test]
fn exits_by_the_result() {
    assert_eq!(exit(Ok(Vec::new())), ExitCode::SUCCESS);
    assert_eq!(exit(Ok(vec!["a".to_string()])), ExitCode::FAILURE);
    assert_eq!(exit(Err("gh: down".to_string())), ExitCode::from(2));
}

#[test]
fn fails_with_no_round_comment() {
    let other = bot("Plan: build it in xtask.");
    assert_eq!(
        check(&record(vec![other])),
        vec![format!(
            "no review round comment by {BOT}. Run `/review` and post each round in \
             the format of .claude/skills/review/SKILL.md, \"Round comment\"."
        )]
    );
}

#[test]
fn ignores_a_round_by_another_account() {
    let pasted = Comment {
        author: "someone".to_string(),
        ..bot(ROUND)
    };
    assert_eq!(
        check(&record(vec![pasted])),
        vec![format!(
            "no review round comment by {BOT}. Run `/review` and post each round in \
             the format of .claude/skills/review/SKILL.md, \"Round comment\"."
        )]
    );
}

#[test]
fn fails_a_range_that_ends_before_a_commit_that_changes_code() {
    let round = second().replace("..c77c67d7", &format!("..{CODE}"));
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![format!(
            "review round 2 ends at `{CODE}`, and the range from it to the head \
             `{HEAD}` changes code at `a.rs:2`. A commit that changes code needs a new \
             round."
        )]
    );
}

#[test]
fn passes_a_range_that_ends_before_commits_that_change_no_code() {
    let round = ROUND.replace("..c77c67d7", &format!("..{TEXT}")).replace(
        "No bug,",
        "Text fixes:\n\n1. In the PR body, write `x`.\n\nNo bug,",
    );
    assert_eq!(check(&reviewed(vec![bot(&round)])), Vec::<String>::new());
    let fields = ROUND.replace("..c77c67d7", &format!("..{TEXT}")).replace(
        "Findings: none",
        "Findings: none\nText fixes:\n- In the PR body, write `x`.",
    );
    assert_eq!(check(&reviewed(vec![bot(&fields)])), Vec::<String>::new());
    let wrapped = ROUND.replace("..c77c67d7", &format!("..{TEXT}")).replace(
        "No bug,",
        "Text fixes:\nin the PR body, write `x`.\n\nNo bug,",
    );
    assert_eq!(check(&reviewed(vec![bot(&wrapped)])), Vec::<String>::new());
}

#[test]
fn fails_a_range_that_ends_before_a_commit_without_a_text_fixes_line() {
    let ended = second().replace("..c77c67d7", &format!("..{TEXT}"));
    let fixes = |line| ended.replace("No bug,", &format!("{line}\n\nNo bug,"));
    for round in [
        ended.clone(),
        fixes("Text fixes: none"),
        fixes("Text fixes: `none`."),
        fixes("Text fixes: none needed"),
        fixes("- Text fixes: x"),
        fixes("> Text fixes: x"),
        ended.replace("Deferred: none", "Deferred: none\nText fixes: x"),
        fixes("Text fixes:\nnone."),
        ended.replace("Findings: none", "Findings: none\nText fixes:\n`none`"),
    ] {
        assert_eq!(
            check(&reviewed(vec![bot(&round)])),
            vec![format!(
                "review round 2 ends at `{TEXT}`, not at the head `{HEAD}`, and has no \
                 `Text fixes:` line."
            )],
            "{round}"
        );
    }
}

#[test]
fn reads_text_fixes_in_the_last_paragraph_of_an_old_round() {
    let round = ROUND
        .replace(END, "\n\nText fixes: in the PR body, write `x`.")
        .replace("..c77c67d7", &format!("..{TEXT}"));
    assert_eq!(check(&reviewed(vec![old(&round)])), Vec::<String>::new());
}

#[test]
fn passes_a_range_that_ends_before_only_clean_merges_without_text_fixes() {
    let round = ROUND.replace("..c77c67d7", &format!("..{MERGED}"));
    assert_eq!(check(&reviewed(vec![bot(&round)])), Vec::<String>::new());
}

#[test]
fn fails_a_range_that_ends_off_the_first_parent_chain() {
    let round = second().replace("..c77c67d7", "..0badc0de");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![format!(
            "review round 2 ends at 0badc0de, not at the head {HEAD}. A commit after \
             the round needs a new round; only a clean merge of the base, or a commit \
             that changes no code after a round with a `Text fixes:` line, does not."
        )]
    );
}

#[test]
fn a_round_3_that_ends_off_the_first_parent_chain_asks_to_correct_its_range() {
    let round = ROUND.replace("..c77c67d7", "..0badc0de");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![format!(
            "review round 3 ends at 0badc0de, which is not a commit on the first-parent \
             chain of the head {HEAD}, or a merge of a branch other than the base \
             follows it. Correct the range of the round comment when it names the \
             wrong commit. Else bring the reviewed commits back onto that chain, with \
             only merges of the base after them."
        )]
    );
}

#[test]
fn a_round_3_before_a_merge_that_is_not_clean_asks_for_a_merge_check() {
    let record = reviewed(vec![bot(ROUND)]);
    let none = |_: &str, _: &str| Ok(None);
    assert_eq!(
        problems(&record, HEAD, &|_| Ok(Place::Unclean), &none, &none),
        Ok(vec![format!(
            "review round 3 ends at c77c67d7, not at the head {HEAD}. Post a round \
             comment numbered 3 that checks the merge of the base that is not clean, \
             and ends at the head."
        )])
    );
}

#[test]
fn a_round_2_before_a_merge_that_is_not_clean_needs_a_new_round() {
    let record = reviewed(vec![bot(&second())]);
    let none = |_: &str, _: &str| Ok(None);
    assert_eq!(
        problems(&record, HEAD, &|_| Ok(Place::Unclean), &none, &none),
        Ok(vec![format!(
            "review round 2 ends at c77c67d7, not at the head {HEAD}. A commit after \
             the round needs a new round; only a clean merge of the base, or a commit \
             that changes no code after a round with a `Text fixes:` line, does not."
        )])
    );
}

#[test]
fn fails_a_round_with_findings() {
    let round = second().replace("Findings: none", "Findings: 1");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![
            "review round 2 has findings (1). Fix or answer them, then run another \
             round."
                .to_string()
        ]
    );
}

/// `ROUND` as a later round of a code PR that does not skip `breaker`.
fn later(reviewers: &str) -> String {
    ROUND
        .replace("Reviewers: reviewer", &format!("Reviewers: {reviewers}"))
        .replace(
            "Breaker: skipped, the range changes no `.rs` line but comments\n",
            "",
        )
}

#[test]
fn fails_a_code_pr_whose_round_names_no_breaker() {
    assert_eq!(
        check(&reviewed(vec![bot(&later("reviewer, architecture"))])),
        vec!["review round 3 names no breaker, which this round requires.".to_string()]
    );
    assert_eq!(
        check(&reviewed(vec![bot(&later("reviewer, breaker"))])),
        Vec::<String>::new()
    );
}

#[test]
fn a_breaker_skip_on_a_range_that_changes_code_fails() {
    let code = ROUND.replace(COMMENTS, "940140aa");
    assert_eq!(
        check(&reviewed(vec![bot(&code)])),
        vec![
            "review round 3 skips `breaker`, but its range changes code at `a.rs:2`."
                .to_string()
        ]
    );
    let named = code.replace("Reviewers: reviewer", "Reviewers: reviewer, breaker");
    assert_eq!(check(&reviewed(vec![bot(&named)])), Vec::<String>::new());
    let first = code.replace("round 3", "round 1");
    assert_eq!(
        check(&record(vec![bot(&first)])),
        vec![
            "review round 1 names no architecture, breaker, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn round_1_of_a_code_pr_needs_each_reviewer_of_the_table() {
    let first = ROUND.replace("round 3", "round 1");
    assert_eq!(
        check(&record(vec![bot(&first)])),
        vec![
            "review round 1 names no architecture, breaker, which this round requires."
                .to_string()
        ]
    );
    let first = first.replace(
        "Reviewers: reviewer",
        "Reviewers: reviewer, architecture, breaker",
    );
    assert_eq!(check(&record(vec![bot(&first)])), Vec::<String>::new());
}

#[test]
fn needs_only_the_reviewer_for_a_diff_with_no_code() {
    let first = later("`reviewer`").replace("round 3", "round 1");
    let mut docs = record(vec![bot(&first)]);
    docs.files = vec![
        "docs/decisions/crate-map.md".to_string(),
        "README.md".to_string(),
    ];
    assert_eq!(check(&docs), Vec::<String>::new());
    for code in ["xtask/Cargo.toml", "Cargo.lock", "a/.rs"] {
        let mut record = record(vec![bot(&first)]);
        record.files = [&docs.files[..], &[code.to_string()]].concat();
        assert_eq!(
            check(&record),
            vec![
                "review round 1 names no architecture, breaker, which this round \
                 requires."
                    .to_string()
            ],
            "{code}"
        );
    }
}

#[test]
fn takes_the_last_round() {
    let later = second().replace("Findings: none", "Findings: 3");
    assert_eq!(
        check(&reviewed(vec![bot(&second()), bot(&later)])),
        vec![
            "review round 2 has findings (3). Fix or answer them, then run another \
             round."
                .to_string()
        ]
    );
}

#[test]
fn each_earlier_round_names_the_reviewers_it_requires() {
    let first = later("reviewer")
        .replace("round 3", "round 1")
        .replace("Findings: none", "Findings: 2");
    assert_eq!(
        check(&record(vec![bot(&first), bot(&later("reviewer, breaker"))])),
        vec![
            "review round 1 names no architecture, breaker, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn reads_the_fields_only_from_the_block_after_the_heading() {
    let round =
        later("reviewer").replace("No bug", "Reviewers: reviewer, breaker\nNo bug");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec!["review round 3 names no breaker, which this round requires.".to_string()]
    );
    let round = ROUND.replace("Range:", "\nRange:");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![
            "review round 3 has no `Range:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
                .to_string()
        ]
    );
}

#[test]
fn names_a_missing_line() {
    let round = ROUND.replace("Findings: none\n", "");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec![
            "review round 3 has no `Findings:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
                .to_string()
        ]
    );
    let round = ROUND.replace("`38cba24f..c77c67d7`", "c77c67d7");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec![
            "review round 3 has the range `c77c67d7`, not `<from>..<head>`".to_string()
        ]
    );
}

fn red(comments: &[&str]) -> Record {
    let mut red = reviewed(vec![bot(ROUND)]);
    red.branch = "red-team/buffer-open-read".to_string();
    red.labels = vec!["oracle".to_string()];
    red.comments.extend(comments.iter().map(|c| bot(c)));
    red
}

#[test]
fn a_red_team_oracle_pr_needs_the_director_approval_at_the_head() {
    let missing = format!(
        "a red-team `oracle` PR needs the director's verdict with the line \
         \"Director: approved at `<sha>`\" for the head {HEAD}."
    );
    assert_eq!(check(&red(&[])), vec![missing.clone()]);
    let earlier = "Quality: 8/10\nGood.\n\nDirector: approved at `82ba5b72`";
    assert_eq!(check(&red(&[earlier])), vec![missing.clone()]);
    let architect = "Quality: 8/10\nGood.\n\nApproved at `c77c67d7`";
    assert_eq!(check(&red(&[architect])), vec![missing.clone()]);
    let short = "Director: approved at `c77c67`";
    assert_eq!(check(&red(&[short])), vec![missing]);
    let head = "Quality: 8/10\nGood.\n\nDirector: approved at `c77c67d7`.";
    assert_eq!(check(&red(&[earlier, head])), Vec::<String>::new());
}

#[test]
fn takes_a_count_of_zero_as_no_findings() {
    let round = ROUND.replace("Findings: none", "Findings: 0");
    assert_eq!(check(&reviewed(vec![bot(&round)])), Vec::<String>::new());
    let round = ROUND.replace("Findings: none", "Findings: few");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![
            "review round 3 has `Findings: few`, not a count or `none`: invalid digit \
             found in string"
                .to_string()
        ]
    );
}

#[test]
fn reads_no_invisible_character_in_a_code_span() {
    for c in ['\u{200B}', '\u{FFF8}'] {
        let head = format!("{NAME}\nQuality: 8/10\n`{c}`");
        assert_eq!(
            check(&record(vec![headed(&head)])),
            unrated("has no summary line after its `Quality:` line"),
            "summary {:X}",
            u32::from(c)
        );
        let comment = ROUND.replace(
            "Hot path: none",
            &format!("`{c}Hot path:` `send`\nHot path: none"),
        );
        assert_eq!(
            check(&record(vec![bot(&comment)])),
            vec!["review round 3 has a second `Hot path:` line in its end lines."],
            "end line {:X}",
            u32::from(c)
        );
    }
}

#[test]
fn a_problem_shows_an_image_in_a_field_that_it_quotes() {
    let round = ROUND.replace("Findings: none", "Findings: 2![](x)");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec![
            "review round 3 has `Findings: 2![]`, not a count or `none`: invalid digit \
             found in string"
        ]
    );
    let round = ROUND.replace("Findings: none", "Findings: ![](x)2![](y)");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec![
            "review round 3 has `Findings: ![]2![]`, not a count or `none`: invalid \
             digit found in string"
        ]
    );
    let round = ROUND.replace("Findings: none", "Findings: **x**gh-1");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec![
            "review round 3 has `Findings: xgh-1`, not a count or `none`: invalid \
             digit found in string"
        ]
    );
    let round = ROUND.replace("`38cba24f..c77c67d7`", "c77c67d7![](x)");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec!["review round 3 has the range `c77c67d7![]`, not `<from>..<head>`"]
    );
    let round = ROUND.replace("round 3", "round 3![](x)");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec!["`## Review round 3![]` has no round number"]
    );
}

#[test]
fn a_problem_shows_an_image_in_the_end_of_a_range_that_it_quotes() {
    let round = second().replace("`38cba24f..c77c67d7`", "38cba24f..c77c67d7![](x)");
    assert_eq!(
        check(&reviewed(vec![bot(&round)])),
        vec![format!(
            "review round 2 ends at c77c67d7![], not at the head {HEAD}. A commit \
             after the round needs a new round; only a clean merge of the base, or a \
             commit that changes no code after a round with a `Text fixes:` line, \
             does not."
        )]
    );
}

#[test]
fn a_problem_of_an_old_round_shows_an_image_in_the_number_that_it_quotes() {
    let round = ROUND
        .replace("round 3", "round 3![](x)")
        .replace("Hot path: none", "Hot path: `send`");
    assert_eq!(
        check(&record(vec![old(&round)])),
        vec![
            "`## Review round 3![]` has no round number",
            "review round 3![] names no performance, which this round requires.",
        ]
    );
}

#[test]
fn a_breaker_skip_problem_shows_an_image_in_a_commit_that_it_quotes() {
    let round = ROUND.replace("`38cba24f..c77c67d7`", "38cba24f![](x)..c77c67d7");
    // The phrase of `History::code_change` for a range start that names no commit.
    let unnamed = |from: &str| format!("has `{from}`, which names no commit");
    assert_eq!(
        problems(
            &reviewed(vec![bot(&round)]),
            HEAD,
            &|_| Ok(Place::Head),
            &|from, end| Ok((end != HEAD).then(|| unnamed(from))),
            &|_, _| Ok(None),
        ),
        Ok(vec![
            "review round 3 skips `breaker`, but its range has `38cba24f![]`, which \
             names no commit."
                .to_string()
        ])
    );
}

#[test]
fn names_a_round_with_no_number() {
    assert_eq!(
        check(&record(vec![bot("## Review round one\nFindings: none")])),
        vec!["`## Review round one` has no round number".to_string()]
    );
}

#[test]
fn fails_a_last_round_in_an_older_format() {
    assert_eq!(
        check(&reviewed(vec![
            bot(ROUND),
            bot("## Review round 4\n\nNo findings.")
        ])),
        vec![
            "review round 4 has no `Range:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
                .to_string()
        ]
    );
    assert_eq!(
        check(&reviewed(vec![bot("## Review round one")])),
        vec!["`## Review round one` has no round number".to_string()]
    );
}

#[test]
fn names_the_pr_field_a_record_lacks() {
    use serde_json::json;
    let pull = [json!({ "head": { "ref": "a" }, "base": {}, "labels": [] })];
    assert_eq!(
        record_of(&pull, &[], &[]).unwrap_err(),
        "JSON has no string field `ref`"
    );
    let pull =
        [json!({ "head": { "ref": "a" }, "base": { "ref": "main" }, "labels": [] })];
    let comment = |created| {
        json!({
            "user": { "login": BOT },
            "body": ROUND,
            "created_at": created,
        })
    };
    let files = [json!({ "filename": "a.rs" })];
    let read = record_of(&pull, &files, &[comment(CUTOFF)]).unwrap();
    assert_eq!((read.branch.as_str(), read.base.as_str()), ("a", "main"));
    assert_eq!(read.files, ["a.rs"]);
    assert_eq!(read.comments[0].author, BOT);
    assert_eq!(read.comments[0].created, CUTOFF);
    for created in [
        "2026-10-08T03:00:00.5Z",
        "2026-10-08 03:00:00Z",
        "2026-1-08T03:00:00Z",
        "2026-10-08T03:00:0aZ",
        "2026-10-08T03:00:00+",
    ] {
        assert_eq!(
            record_of(&pull, &files, &[comment(created)]).unwrap_err(),
            format!(
                "`created_at` is `{created}`, not a UTC time `YYYY-MM-DDTHH:MM:SSZ`"
            )
        );
    }
}

#[test]
fn a_breaker_skip_on_a_range_that_changes_only_a_script_fails() {
    let script = |from: &str, _: &str| {
        Ok((from == COMMENTS).then(|| "changes code at `a.sh:1`".to_string()))
    };
    assert_eq!(
        problems(
            &reviewed(vec![bot(ROUND)]),
            HEAD,
            &|_| Ok(Place::Head),
            &script,
            &|_, _| Ok(None)
        ),
        Ok(vec![
            "review round 3 skips `breaker`, but its range changes code at `a.sh:1`."
                .to_string()
        ])
    );
}

#[test]
fn a_red_team_pr_with_no_oracle_label_needs_no_approval() {
    let mut red = red(&[]);
    red.labels.clear();
    assert_eq!(check(&red), Vec::<String>::new());
}

#[test]
fn returns_a_failure_to_read_history() {
    let failed = |_: &str| Err("git rev-parse: bad".to_string());
    assert_eq!(
        problems(
            &record(vec![bot(&second())]),
            HEAD,
            &failed,
            &|_, _| Ok(None),
            &|_, _| Ok(None)
        ),
        Err("git rev-parse: bad".to_string())
    );
    let failed = |_: &str, _: &str| Err("git diff: bad".to_string());
    assert_eq!(
        problems(
            &record(vec![bot(&second())]),
            HEAD,
            &|_| Ok(Place::Head),
            &failed,
            &|_, _| Ok(None)
        ),
        Err("git diff: bad".to_string())
    );
}

#[test]
fn a_fence_with_a_no_break_space_after_it_does_not_close() {
    let quoted = ROUND.replace("weakening.\n\n", "weakening.\n\n```\n```\u{a0}\n\n");
    assert_ne!(quoted, ROUND);
    assert_eq!(
        check(&reviewed(vec![bot(&quoted)])),
        vec![unended("Deferred")]
    );
    let shown = later("reviewer, breaker").replace(
        "weakening.\n\n",
        "weakening.\n\nDeferred: none\nPublic surface: none\nHot path: `send`\n\n\
         ```\n```\u{a0}\n\n",
    );
    assert_ne!(shown, later("reviewer, breaker"));
    assert_eq!(
        check(&reviewed(vec![bot(&shown)])),
        vec![unended("Deferred")]
    );
    let closed = ROUND.replace("weakening.\n\n", "weakening.\n\n```\n``` \t\n\n");
    assert_eq!(check(&reviewed(vec![bot(&closed)])), Vec::<String>::new());
}

#[test]
fn an_old_malformed_round_that_names_performance_needs_no_performance() {
    let named = old(
        "## Review round 1\n\nReviewers: reviewer, architecture, breaker, performance\n\
         Range: a\nFindings: 2\n\nHot path: `send`",
    );
    assert_eq!(
        check(&record(vec![named, bot(ROUND)])),
        vec!["review round 1 has the range `a`, not `<from>..<head>`".to_string()]
    );
    let free = old(
        "## Review round 1\n\nConfirmed findings.\n\nReviewers: reviewer, \
         architecture, breaker, performance\n\nHot path: `send`",
    );
    assert_eq!(check(&record(vec![free, bot(ROUND)])), Vec::<String>::new());
}

#[test]
fn an_indented_reviewers_line_of_an_old_round_names_no_performance() {
    let indented = old("## Review round 1\n\nConfirmed findings.\n\n    \
         Reviewers: reviewer, performance\n\nHot path: `send`");
    assert_eq!(
        check(&record(vec![indented, bot(ROUND)])),
        vec![
            "review round 1 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn a_lone_carriage_return_ends_a_line() {
    // "```\r\r\n" is a closing fence and a blank line, so the last fence hides the
    // last end lines, and the end lines GitHub shows name `send`.
    let hidden = later("reviewer, breaker").replace(
        "weakening.\n\n",
        "weakening.\n\n```\ncode\n```\r\r\n\
         Deferred: none\nPublic surface: none\nHot path: `send`\n\n```\n\n",
    );
    assert_ne!(hidden, later("reviewer, breaker"));
    assert_eq!(
        check(&reviewed(vec![bot(&hidden)])),
        vec![unended("Deferred")]
    );
    let crlf = ROUND.replace('\n', "\r\n");
    assert_eq!(check(&reviewed(vec![bot(&crlf)])), Vec::<String>::new());
    let cr = ROUND.replace('\n', "\r");
    assert_eq!(check(&reviewed(vec![bot(&cr)])), Vec::<String>::new());
}

#[test]
fn a_parsed_old_round_reads_its_reviewers_field_at_any_indent() {
    let indented = ROUND
        .replace(
            "\nReviewers: reviewer\n",
            "\nRange: `38cba24f..c77c67d7`\n    Reviewers: reviewer, performance\n",
        )
        .replace("\nBreaker:", "\n    Breaker:")
        .replace(
            "\nRange: `38cba24f..c77c67d7`\nFindings:",
            "\n    Findings:",
        )
        .replace("Hot path: none", "Hot path: `send`");
    assert_ne!(indented, ROUND);
    assert_eq!(check(&reviewed(vec![old(&indented)])), Vec::<String>::new());
    let code = ROUND
        .replace(
            "\nReviewers: reviewer\n",
            "\n    Reviewers: reviewer, performance\n",
        )
        .replace("\nBreaker:", "\n    Breaker:")
        .replace("\nRange:", "\n    Range:")
        .replace("\nFindings:", "\n    Findings:")
        .replace("Hot path: none", "Hot path: `send`");
    assert_eq!(
        check(&reviewed(vec![old(&code)])),
        vec![
            "review round 3 has no `Range:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
                .to_string(),
            "review round 3 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn a_parsed_old_round_reads_performance_only_from_its_reviewers_field() {
    let quoted = ROUND
        .replace(
            "weakening.\n\n",
            "weakening.\n\nReviewers: reviewer, performance\n\n",
        )
        .replace("Hot path: none", "Hot path: `send`");
    assert_ne!(quoted, ROUND);
    assert_eq!(
        check(&reviewed(vec![old(&quoted)])),
        vec![
            "review round 3 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn an_old_round_reads_no_line_of_an_indented_code_block() {
    let malformed = |reviewers: &str, hot: &str| {
        old(&format!(
            "## Review round 1\n\nConfirmed findings.\n\n{reviewers}Reviewers: \
             reviewer, performance\n\n{hot}Hot path: `send`"
        ))
    };
    let unnamed = vec![
        "review round 1 names no performance, which this round requires.".to_string(),
    ];
    let cases = [
        ("   ", "", Vec::new()),
        ("    ", "", unnamed.clone()),
        ("\t", "", unnamed.clone()),
        ("    ", "   ", unnamed),
        ("    ", "    ", Vec::new()),
        ("    ", "\t", Vec::new()),
    ];
    for (reviewers, hot, problems) in cases {
        let comment = malformed(reviewers, hot);
        assert_eq!(
            check(&record(vec![comment, bot(ROUND)])),
            problems,
            "{reviewers:?} {hot:?}"
        );
    }
}

#[test]
fn a_lone_carriage_return_ends_a_line_of_an_approval() {
    let head = "Quality: 8/10\rGood.\r\rDirector: approved at `c77c67d7`.";
    assert_eq!(check(&red(&[head])), Vec::<String>::new());
}

#[test]
fn fails_a_round_with_raw_html() {
    let cases = [
        ("<div>\n```\n</div>", "<div>"),
        ("<!--\nHot path: none\n-->", "<!--"),
        ("<div><!--", "<div><!--"),
        ("<source\n***", "<source"),
        ("- a\n  <source", "<source"),
        ("A <details> b", "A <details> b"),
        ("A `b`\nc <!-- d --> e", "c <!-- d --> e"),
        ("</x>", "</x>"),
        ("<?a ?>", "<?a ?>"),
        ("<source\n---", "<source"),
        ("<source\n===", "<source"),
        ("a\n<source\n---", "<source"),
        ("<source | b\n--- | ---\nc | d", "<source | b"),
        ("> 1. <source", "> 1. <source"),
        ("- <source", "- <source"),
        ("- ```\n  a\n  ```\n  <source", "<source"),
        ("| a |\n| - |\n<source", "<source"),
        ("</source", "</source"),
        ("<! a", "<! a"),
        ("+ <source", "+ <source"),
        ("* <source", "* <source"),
        ("1) <source", "1) <source"),
        ("-\t<source", "-\t<source"),
        ("[^1]: <source", "[^1]: <source"),
        ("[^1]: [^2]: <source x", "[^1]: [^2]: <source x"),
        ("> [^a]: <source", "> [^a]: <source"),
        ("[^a\\\\]: <source", "[^a\\\\]: <source"),
        ("- [^a]:<source", "- [^a]:<source"),
        ("[^a\\]: <source", "[^a\\]: <source"),
        ("[^\\]: <source", "[^\\]: <source"),
        ("[^a[b]: <source", "[^a[b]: <source"),
        ("[^`\\]: <source x`", "[^`\\]: <source x`"),
    ];
    for (html, line) in cases {
        let comment =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{html}\n\n"));
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&record(vec![bot(&comment)])),
            vec![raw(line)],
            "{html}"
        );
    }
    let summary = format!("<b>\n\n{ROUND}");
    assert_eq!(check(&record(vec![bot(&summary)])), vec![raw("<b>")]);
    let hidden = ROUND.replace("weakening.\n\n", "weakening.\n\n<source\n---\n");
    assert_eq!(check(&record(vec![bot(&hidden)])), vec![raw("<source")]);
    let noted = ROUND.replace("weakening.\n\n", "weakening.[^1]\n\n[^1]: <source\n\n");
    assert_eq!(
        check(&record(vec![bot(&noted)])),
        vec![raw("[^1]: <source")]
    );
}

#[test]
fn fails_raw_html_before_the_fields_or_the_heading() {
    let unranged = ROUND
        .replace("Range: `38cba24f..c77c67d7`\n", "")
        .replace("weakening.\n\n", "weakening.\n\n<div>\n\n");
    assert_eq!(check(&record(vec![bot(&unranged)])), vec![raw("<div>")]);
    let hidden = ROUND.replace("\nReviewers:", "\n<details>\n\nReviewers:");
    assert_eq!(check(&record(vec![bot(&hidden)])), vec![raw("<details>")]);
    for heading in [
        "<search\n## Review round 3",
        "<search\n   ## Review round 3",
        "<search\n## Review round 3\n## Review round 4",
        "<search\r## Review round 3",
        "<search\r## Review round 3\r## Review round 4",
        "<search\n## Review *round* 3",
        "<search\n## Review&#32;round 3",
        "<search\n## Review  round 3",
        "<search\n## Review [round][r] 3\n[r]: /x",
        "<search\n***\nReview round 3\n---",
        "<search\n## x\nReview round 3\n---",
        "<search\n<search\n## Review *round* 3",
        "<search\n## Review [round][d] 3",
        " <search\n <search\n## Review *round* 3",
        "<search\n## Review round 3\n\n<search\n## Review round 4",
        "<search\n## x\n<b></b>Review round 3\n---",
        "<search\n***\n<i></i>Review round 3\n---",
    ] {
        let headless = ROUND.replace("## Review round 3", heading) + "\n\n[d]: /x";
        assert_eq!(
            check(&record(vec![bot(&headless)])),
            vec![raw("<search")],
            "{heading}"
        );
    }
}

#[test]
fn fails_raw_html_before_a_heading_that_github_shows_and_comrak_does_not() {
    let found = ROUND.replace("Findings: none", "Findings: 2");
    let notes = "x[^1]\n\n[^1]: <search\n    <search\n    <search\n\n";
    for (front, heading, line) in [
        ("", "<search>Review round 3\n---", "<search>Review round 3"),
        ("", "<search\n<!-- x -->\nReview round 3\n---", "<search"),
        (
            "",
            "<SEARCH x\n<!-- x -->\nReview round 3\n---",
            "<SEARCH x",
        ),
        ("", "<!doctype x>\n## Review round 3", "<!doctype x>"),
        ("", "<search\n<?a?>\nReview round 3\n---", "<search"),
        ("", "<search\n<!----- x -->\nReview round 3\n---", "<search"),
        ("", "<!-- a -->\nReview round 3\n---", "<!-- a -->"),
        ("", "- <source x\nReview round 3\n---", "- <source x"),
        ("", "> <source x\nReview round 3\n---", "> <source x"),
        ("", "- <source\nReview round 3\n---", "- <source"),
        ("", "- </SOURCE>a\nReview round 3\n---", "- </SOURCE>a"),
        ("", "<source>a\n<textarea\n\n## Review round 3", "<source>a"),
        ("", "<search\ta\n## Review round 3", "<search\ta"),
        ("", "<search\u{b}a\n## Review round 3", "<search\u{b}a"),
        ("", "<search\u{c}a\n## Review round 3", "<search\u{c}a"),
        ("", "x\n<search>\n## Review round 3", "<search>"),
        (
            "",
            "</search x=\"1\">\n## Review round 3",
            "</search x=\"1\">",
        ),
        ("", "<!x\n<!x\n## Review round 3", "<!x"),
        (
            "",
            "<search\n## Review round 2\n\n## Review round 3",
            "<search",
        ),
        (notes, "a\n<search\n## Review round 3", "[^1]: <search"),
        (notes, "é\n<search\n## Review round 3", "[^1]: <search"),
    ] {
        let last = front.to_owned() + &found.replace("## Review round 3", heading);
        assert_eq!(
            check(&reviewed(vec![bot(ROUND), bot(&last)])),
            vec![raw(line)],
            "{heading}"
        );
    }
}

#[test]
fn fails_raw_html_after_a_byte_order_mark_at_the_start() {
    let found = ROUND.replace("Findings: none", "Findings: 2");
    let at = found.find("## Review round 3").unwrap();
    for line in ["<search", "<!x", "<SEARCH x", "<source", "<SOURCE x"] {
        let last = format!("\u{feff}{line}\n{}", &found[at..]);
        assert_eq!(
            check(&reviewed(vec![bot(ROUND), bot(&last)])),
            vec![raw(line)],
            "{line}"
        );
    }
}

#[test]
fn passes_a_code_block_after_a_byte_order_mark_at_the_start() {
    let at = ROUND.find("## Review round 3").unwrap();
    let last = format!("\u{feff}    <b>x</b>\n\n{}", &ROUND[at..]);
    assert_eq!(
        check(&reviewed(vec![bot(ROUND), bot(&last)])),
        Vec::<String>::new()
    );
}

#[test]
fn reads_no_heading_after_two_byte_order_marks() {
    // GitHub drops only the first mark, so it shows the heading line as text.
    let earlier = second()
        .replace("Findings: none", "Findings: 2")
        .replace("## Review round 2", "## Review round 1");
    let at = second().find("## Review round 2").unwrap();
    for (marks, expected) in [
        ("\u{feff}", vec![]),
        (
            "\u{feff}\u{feff}",
            vec![
                "review round 1 has findings (2). Fix or answer them, then run another \
                 round."
                    .to_owned(),
            ],
        ),
    ] {
        let last = format!("{marks}{}", &second()[at..]);
        assert_eq!(check(&reviewed(vec![bot(&earlier), bot(&last)])), expected);
    }
    // After the second mark, a tag is text, and the heading after it shows.
    let shown = check(&reviewed(vec![bot(&second()), bot(&earlier[at..])]));
    assert_eq!(shown.len(), 1);
    for line in ["<search x", "<!-- x", "<SEARCH x"] {
        let last = format!("\u{feff}\u{feff}{line}\n{}", &earlier[at..]);
        assert_eq!(
            check(&reviewed(vec![bot(&second()), bot(&last)])),
            shown,
            "{line}"
        );
    }
}

#[test]
fn passes_a_round_whose_heading_github_hides_in_an_html_block() {
    let found = ROUND.replace("Findings: none", "Findings: 2");
    // GitHub starts a block of type 7 at a complete tag alone on its line.
    for heading in [
        "<search>\n## Review round 3",
        "<search x=\"1\">\n## Review round 3",
        "</search>\n<!-- x -->\nReview round 3\n---",
        "<search/>\n<!-- x -->\nReview round 3\n---",
    ] {
        let last = found.replace("## Review round 3", heading);
        assert_eq!(
            check(&reviewed(vec![bot(ROUND), bot(&last)])),
            Vec::<String>::new(),
            "{heading}"
        );
    }
}

#[test]
fn passes_a_round_whose_text_github_shows_as_text() {
    let shown = [
        "<https://github.com>",
        "\\<div>",
        "`Vec<u8>`",
        "```\n<div>\n```",
        "    <div>",
        "a < b, <1",
        "# <source",
        "a | <source\n--- | ---",
        "*a*<source",
        "*<source",
        "1.<source",
        "a <b",
        "[^]: <source",
        "[^a]b]: <source",
        "[^a\\]b]: <source",
        "[^a b]: <source",
        "[^a\tb]: <source",
        "[^a] <source",
        "[^a]<source",
    ];
    for text in shown {
        let comment =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{text}\n\n"));
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            Vec::<String>::new(),
            "{text}"
        );
    }
    let old = old("## Review round 1\n\nNo fields.\n\n<div>");
    assert_eq!(
        check(&reviewed(vec![old, bot(ROUND)])),
        Vec::<String>::new()
    );
}

/// The problem of a round 3 whose fields have no `Range:` line.
fn rangeless() -> String {
    "review round 3 has no `Range:` line. Write the round in the format of \
     .claude/skills/review/SKILL.md, \"Round comment\"."
        .to_string()
}

#[test]
fn hides_the_lines_of_a_footnote_with_no_reference() {
    // GitHub reads each label as a footnote, which continues onto the fields, and
    // hides it.
    let fields = [
        ROUND.replace("\nReviewers:", "\n[^a\\]: x\nReviewers:"),
        ROUND.replace(
            "## Review round 3\n\n",
            "## Review round 3\n\n[x]: /u \"t\n[^a\\]: y\"\n",
        ),
    ];
    let spans = [
        "[x](https://x.y \"t\n[^a\\]: y\")",
        "`x\n[^a\\]: y`",
        "> `x\n> [^a\\]: y`",
        "*a `x\n[^a\\]: y` b*",
    ]
    .map(|text| ROUND.replace("but comments", &format!("but {text}")));
    for hidden in fields.iter().chain(&spans) {
        assert_ne!(hidden, ROUND);
        assert_eq!(
            check(&record(vec![bot(hidden)])),
            vec![rangeless()],
            "{hidden}"
        );
    }
    let end = ROUND.replace("weakening.\n\n", "weakening.\n\n    c\r[^a\\]: x\n");
    assert_ne!(end, ROUND);
    assert_eq!(check(&record(vec![bot(&end)])), vec![unended("Deferred")]);
}

#[test]
fn passes_a_footnote_with_no_reference_that_hides_no_field() {
    let cases = [
        "[^a\\]: x",
        "[^\\]: x",
        "[^a[b]: x",
        "> [^a\\]: x",
        "- [^a\\]: x",
        "[^1]: [^a\\]: x",
        "[^1]: [^2]: [^a\\]: x",
        "a\n[^a\\]: x",
        "[^a\\]: x](https://x.y)",
        "a `x\n  [^a\\]: y`",
        "> a `x\n> [^a\\]: y`",
        "- a `x\n  [^a\\]: y`",
        "[^1]: - [^a\\]: x",
        "[^1]: > [^a\\]: x",
        "a\r[^a\\]: x",
        "[^1]: x",
        "> [^a]: x",
        "[^a]: x\n[^b]: y",
        "[^a\\\\]:x",
        "`[^a\\]: x`",
        "```\n[^a\\]: x\n```",
        "> ```\n> [^a\\]: x\n> ```",
        "    [^a\\]: x",
        "- a\n\n      [^a\\]: x",
    ];
    for text in cases {
        let comment =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{text}\n\n"));
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            Vec::<String>::new(),
            "{text}"
        );
    }
    let first = format!("[^a\\]: x\n\n{ROUND}");
    assert_eq!(check(&reviewed(vec![bot(&first)])), Vec::<String>::new());
}

#[test]
fn fails_raw_html_on_a_line_inside_a_span() {
    let cases = [
        ("a `x\n<source y` z", "<source y` z"),
        ("a `x\n<div>` b", "<div>` b"),
        ("a `x\n> <source y`", "> <source y`"),
        (
            "a `x\n<source [y](https://x.y)`",
            "<source [y](https://x.y)`",
        ),
        ("[a\n<source y](https://x.y)", "<source y](https://x.y)"),
    ];
    for (text, line) in cases {
        let hidden = ROUND.replace("but comments", &format!("but {text}"));
        assert_ne!(hidden, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&hidden)])),
            vec![raw(line)],
            "{text}"
        );
    }
    let definition = ROUND.replace(
        "## Review round 3\n\n",
        "## Review round 3\n\n[x]: /u \"t\n<source y\"\n\n",
    );
    assert_ne!(definition, ROUND);
    assert_eq!(
        check(&reviewed(vec![bot(&definition)])),
        vec![raw("<source y\"")]
    );
    for text in ["`x\ny <source z`", "`x\n<https://x.y>`", "`x\n< y`"] {
        let shown = ROUND.replace("but comments", &format!("but {text}"));
        assert_ne!(shown, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&shown)])),
            Vec::<String>::new(),
            "{text}"
        );
    }
}

#[test]
fn reads_a_line_after_a_tab_that_a_list_item_takes_in_part() {
    // The item takes 3 of the 4 columns of the tab, and the line continues the quote.
    let hot = old("## Review round 1\n\nNo fields.\n\n*  > a\n\tHot path: `send`");
    assert_eq!(
        check(&record(vec![hot, bot(ROUND)])),
        vec!["review round 1 names no performance, which this round requires."]
    );
    let wide = ROUND.replace("weakening.\n\n", "weakening.\n\n*  > a\n\t\u{e9}\n\n");
    assert_ne!(wide, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&wide)])), Vec::<String>::new());
}

#[test]
fn fails_a_round_with_a_footnote_reference_over_a_line_break() {
    // GitHub shows each `[^` up to a `]` on a later line as `[^]`.
    // A `]` that a `[` before it takes, an escape, or an entity does not end it.
    for mark in ["[^x", "[^x [y]", "[^x \\]", "[^x\\]", "[^x &#93;"] {
        let end = ROUND.replace(
            "Public surface: none\n",
            &format!("Public surface: none {mark}\n"),
        );
        let end = end.replace("Hot path: none", "Hot path: none ]");
        let line = format!("Public surface: none {mark}");
        assert_eq!(
            check(&record(vec![bot(&end)])),
            vec![bracket(&line)],
            "{mark}"
        );
    }
    for (text, line) in [
        ("a [^*f*\ng]", "a [^*f*"),
        ("a [^x `]`\nb ]", "a [^x `]`"),
        ("a [^x] [^y\nz]", "a [^x] [^y"),
        ("- a\n\n  b [^x\n  c ]", "b [^x"),
    ] {
        let hidden =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{text}\n\n"));
        assert_ne!(hidden, ROUND);
        assert_eq!(
            check(&record(vec![bot(&hidden)])),
            vec![bracket(line)],
            "{text}"
        );
    }
}

#[test]
fn passes_a_footnote_mark_in_a_link_or_a_code_span() {
    for text in [
        "a [^x\nb](u)",
        "a `[^x`",
        "```\n[^x\n```",
        "a [^] [^x]",
        "a [^x *y* z]",
        "a\nb [^x",
    ] {
        let shown =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{text}\n\n"));
        assert_ne!(shown, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&shown)])),
            Vec::<String>::new(),
            "{text}"
        );
    }
}

#[test]
fn hides_both_footnotes_with_no_reference_of_one_label() {
    let notes = format!("{ROUND}\n\n[^a]: x\n\n[^a]: y");
    assert_eq!(check(&reviewed(vec![bot(&notes)])), Vec::<String>::new());
}

#[test]
fn an_old_round_reads_the_first_footnote_of_a_label_with_a_reference() {
    // GitHub shows the first definition of a label, also in another case.
    for second in ["[^a]: y", "[^A]: y"] {
        let round = old(&format!(
            "## Review round 1\n\nConfirmed a finding [^a].\n\n\
             [^a]: Hot path: `send`\n\n{second}"
        ));
        assert_eq!(
            check(&record(vec![round, bot(ROUND)])),
            vec!["review round 1 names no performance, which this round requires."],
            "{second}"
        );
    }
    let round = old(
        "## Review round 1\n\nConfirmed a finding [^a].\n\n[^a]: x\n\n\
         [^a]: Hot path: `send`",
    );
    assert_eq!(
        check(&record(vec![round, bot(ROUND)])),
        Vec::<String>::new()
    );
    let round = old(
        "## Review round 1\n\nConfirmed a finding [^a] [^b].\n\n[^a]: x\n\n\
         [^b]: Hot path: `send`",
    );
    assert_eq!(
        check(&record(vec![round, bot(ROUND)])),
        vec!["review round 1 names no performance, which this round requires."]
    );
}

#[test]
fn an_old_round_reads_a_tab_after_a_quote_mark_to_the_next_multiple_of_4() {
    // Each line starts at an odd offset.
    let round = |text: &str| {
        old(&format!(
            "## Review round 1\n\nConfirmed a finding.\n\n{text}"
        ))
    };
    let unnamed = vec![
        "review round 1 names no performance, which this round requires.".to_string(),
    ];
    // The quote takes the space, and the tab is 2 columns of indent.
    let quoted = round("> \tHot path: `send`");
    assert_eq!(check(&record(vec![quoted, bot(ROUND)])), unnamed);
    // A lazy line of the inner quote, whose text starts with `é`.
    let lazy = round("> > a\n>\t\u{e9}Hot path: `send`");
    assert_eq!(check(&record(vec![lazy, bot(ROUND)])), Vec::<String>::new());
}

#[test]
fn passes_a_code_block_in_a_footnote_with_no_reference() {
    for note in ["```\n    <b>\n    ```", "    <b>"] {
        let hidden = format!("{ROUND}\n\n[^a]: x\n\n    {note}");
        assert_eq!(
            check(&reviewed(vec![bot(&hidden)])),
            Vec::<String>::new(),
            "{note}"
        );
    }
}

#[test]
fn names_the_first_line_that_can_hide_text() {
    for (text, problem) in [
        ("<b>\n\n<source x", raw("<b>")),
        ("<source x\n\n<b>", raw("<source x")),
        ("[^x\nb ]\n\n<b>", bracket("[^x")),
        ("<b>\n\n[^x\nb ]", raw("<b>")),
        ("a[^1]\n\n[^1]: a <b>\n\nc <i>", raw("[^1]: a <b>")),
        (
            "[x](https://x.y \"t\n<source y\")",
            misplaced("[x](https://x.y \"t"),
        ),
    ] {
        let hidden =
            ROUND.replace("weakening.\n\n", &format!("weakening.\n\n{text}\n\n"));
        assert_ne!(hidden, ROUND);
        assert_eq!(check(&record(vec![bot(&hidden)])), vec![problem], "{text}");
    }
}

#[test]
fn fails_a_paragraph_that_comrak_places_in_the_wrong_lines() {
    // GitHub shows `Findings: 2` and the second `Hot path:` line. comrak places each
    // line after the link or the definition too early.
    let fields = ROUND.replace(
        "Reviewers: reviewer\n",
        "Reviewers: reviewer, [x](https://x.y \"a\nRange: `38cba24f..c77c67d7`\n\
         Findings: none\n\")\nFindings: 2\n",
    );
    let end = ROUND.replace(
        "Deferred: none",
        "Deferred: none [x](https://x.y \"a\nPublic surface: none\nHot path: none\n\")",
    );
    let end = format!("{end}\nHot path: `send`");
    let link = ROUND.replace("Deferred: none", "Deferred: none [x](\nhttps://x.y)");
    let definition =
        ROUND.replace("Deferred: none", "[x]: https://x.y\nDeferred: none");
    for (comment, line) in [
        (fields, "Reviewers: reviewer, [x](https://x.y \"a"),
        (end, "Deferred: none [x](https://x.y \"a"),
        (link, "Deferred: none [x]("),
        (definition, "[x]: https://x.y"),
    ] {
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            vec![misplaced(line)],
            "{comment}"
        );
    }
    let text = ROUND.replace("Deferred: none", "Deferred: none [x\ny](https://x.y)");
    assert_eq!(check(&reviewed(vec![bot(&text)])), Vec::<String>::new());
    // comrak places the `\\x` span inside the `é` of the line before.
    let wide =
        ROUND.replace("weakening.\n\n", "weakening.\n\n[r]: u\n\u{e9}\n\\\\x\n\n");
    assert_eq!(
        check(&reviewed(vec![bot(&wide)])),
        vec![misplaced("[r]: u")]
    );
    // The one paragraph after the heading is both the fields and the end lines.
    let alone = bot("## Review round 3\n\n[r]: https://x.y\nz");
    assert_eq!(
        check(&record(vec![alone])),
        vec![misplaced("[r]: https://x.y")]
    );
}

#[test]
fn fails_an_old_round_with_a_paragraph_that_comrak_places_in_the_wrong_lines() {
    // GitHub shows `Hot path: send` as a line of text in the first three cases, hides
    // `Reviewers: performance` in a link title in the fourth, and shows
    // `2. Hot path: send` in the fifth.
    let performance = "review round 1 names no performance, which this round requires.";
    for (text, line, named) in [
        (
            "[r]: https://x.y\nHot path: `send`",
            "[r]: https://x.y",
            false,
        ),
        (
            "See [x](https://x.y \"a\nb\")\nHot path: `send`",
            "See [x](https://x.y \"a",
            false,
        ),
        (
            "> - See [x](\n>   https://x.y)\n>   Hot path: `send`",
            "> - See [x](",
            false,
        ),
        (
            "See [x](https://x.y \"a\nReviewers: performance\nb\")\nmore\n\n\
             Hot path: `send`",
            "See [x](https://x.y \"a",
            true,
        ),
        (
            "[r]: https://x.y\n2. Hot path: `send`",
            "[r]: https://x.y",
            false,
        ),
        // Raw HTML before the paragraph does not count in an old round.
        (
            "<b>a</b>\n\nSee [x](\nhttps://x.y)\nHot path: `send`",
            "See [x](",
            false,
        ),
    ] {
        let round = old(&format!(
            "## Review round 1\n\nConfirmed a finding.\n\n{text}"
        ));
        let mut problems = vec![misplaced(line).replace("round 3", "round 1")];
        problems.extend(named.then(|| performance.to_string()));
        assert_eq!(check(&record(vec![round, bot(ROUND)])), problems, "{text}");
    }
    let round = old("## Review round x\n\nConfirmed a finding.\n\n[r]: https://x.y\nz");
    assert_eq!(
        check(&record(vec![round, bot(ROUND)])),
        vec!["`## Review round x` has no round number"]
    );
}

#[test]
fn fails_raw_html_after_a_lone_carriage_return_in_a_code_block() {
    let hidden = ROUND.replace("weakening.\n\n", "weakening.\n\n    c\r<!-- x\n");
    assert_ne!(hidden, ROUND);
    assert_eq!(check(&record(vec![bot(&hidden)])), vec![raw("<!-- x")]);
}

#[test]
fn fails_end_lines_that_a_fence_after_a_list_item_hides() {
    let hidden = later("reviewer, breaker").replace(
        "weakening.\n\n",
        "weakening.\n\nDeferred: none\nPublic surface: none\nHot path: `send`\n\n\
         - ```\n  x\n  ```\n\n```\n\n",
    );
    assert_ne!(hidden, later("reviewer, breaker"));
    assert_eq!(
        check(&record(vec![bot(&hidden)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn an_old_round_reads_each_line_of_text_that_github_shows() {
    let unnamed = vec![
        "review round 1 names no performance, which this round requires.".to_string(),
    ];
    let shown = [
        "Confirmed findings.\n    {line}",
        "Confirmed findings.\n\t{line}",
        "- a\n  - b\n\n    {line}",
        "Confirmed[^1].\n\n[^1]: a\n    {line}",
        "> Confirmed.\n> {line}",
        "- {line}",
        "- [ ] {line}",
    ];
    for text in shown {
        let hot = text.replace("{line}", "Hot path: `send`");
        let comment = old(&format!("## Review round 1\n\nNo fields.\n\n{hot}"));
        assert_eq!(check(&record(vec![comment, bot(ROUND)])), unnamed, "{hot}");
        let named = text.replace("{line}", "Reviewers: reviewer, performance");
        let comment = old(&format!(
            "## Review round 1\n\nNo fields.\n\n{named}\n\nHot path: `send`"
        ));
        assert_eq!(
            check(&record(vec![comment, bot(ROUND)])),
            Vec::<String>::new(),
            "{named}"
        );
    }
}

#[test]
fn an_old_round_reads_no_line_of_a_fence_in_a_nested_list_item() {
    let fenced = |line: &str| {
        old(&format!(
            "## Review round 1\n\n- a\n  - b\n\n    ```\n    {line}\n    ```"
        ))
    };
    assert_eq!(
        check(&record(vec![fenced("Hot path: `send`"), bot(ROUND)])),
        Vec::<String>::new()
    );
    let named =
        fenced("Reviewers: reviewer, performance").body + "\n\nHot path: `send`";
    assert_eq!(
        check(&record(vec![old(&named), bot(ROUND)])),
        vec![
            "review round 1 names no performance, which this round requires."
                .to_string()
        ]
    );
}

#[test]
fn a_fence_with_a_tab_after_it_closes() {
    let hidden = later("reviewer, breaker").replace(
        "weakening.\n\n",
        "weakening.\n\n```\n```\t\n\nDeferred: none\nPublic surface: none\n\
         Hot path: `send`\n\n```\n\n",
    );
    assert_ne!(hidden, later("reviewer, breaker"));
    assert_eq!(
        check(&record(vec![bot(&hidden)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn reads_the_blocks_of_the_extensions_of_github() {
    let table =
        old("## Review round 1\n\nNo fields.\n\nHot path: `send` | a\n--- | ---");
    assert_eq!(
        check(&record(vec![table, bot(ROUND)])),
        Vec::<String>::new()
    );
    let unreferenced = ROUND.to_string() + "\n\n[^1]: Hot path: `send`";
    assert_eq!(
        check(&reviewed(vec![bot(&unreferenced)])),
        Vec::<String>::new()
    );
    let footnote = unreferenced.replace("weakening.", "weakening.[^1]");
    assert_ne!(footnote, unreferenced);
    assert_eq!(
        check(&reviewed(vec![bot(&footnote)])),
        vec![unended("Deferred")]
    );
    let early = ROUND.replace("weakening.\n\n", "weakening.[^1]\n\n[^1]: a\n\n");
    assert_ne!(early, ROUND);
    assert_eq!(
        check(&reviewed(vec![bot(&early)])),
        vec![unended("Deferred")]
    );
    let rule = ROUND.to_string() + "\n\n***";
    assert_eq!(
        check(&reviewed(vec![bot(&rule)])),
        vec![unended("Deferred")]
    );
}

#[test]
fn reads_inline_markup_as_part_of_its_line() {
    let named = format!("**laptop.integrator-2** · author\n{ROUND}")
        .replace("Deferred: none", "Deferred: *none*");
    assert_eq!(check(&reviewed(vec![bot(&named)])), Vec::<String>::new());
    for markup in ["*a*", "**a**", "~~a~~", "[a](b)", "![a](b)"] {
        let glued =
            format!("## Review round 1\n\nNo fields.\n\n{markup}Hot path: `send`");
        assert_eq!(
            check(&reviewed(vec![old(&glued), bot(ROUND)])),
            Vec::<String>::new(),
            "{markup}"
        );
    }
}

#[test]
fn reads_a_line_after_a_break_or_a_block_as_its_own() {
    let broken = ROUND.replace("Deferred: none\n", "Deferred: none\\\n");
    assert_ne!(broken, ROUND);
    assert_eq!(check(&reviewed(vec![bot(&broken)])), Vec::<String>::new());
    for item in [
        "- a\n  ***\n  Hot path: `send`",
        "- a\n  - Hot path: `send`",
    ] {
        let comment = old(&format!("## Review round 1\n\nNo fields.\n\n{item}"));
        assert_eq!(
            check(&record(vec![comment, bot(ROUND)])),
            vec!["review round 1 names no performance, which this round requires."],
            "{item}"
        );
    }
}

#[test]
fn reads_no_fields_in_a_list() {
    let listed = ROUND.replace("\nReviewers:", "\n- x\n  Reviewers:");
    assert_ne!(listed, ROUND);
    assert_eq!(
        check(&record(vec![bot(&listed)])),
        vec![
            "review round 3 has no `Range:` line. Write the round in the format of \
             .claude/skills/review/SKILL.md, \"Round comment\"."
        ]
    );
}

#[test]
fn reads_only_a_top_level_round_heading() {
    for quoted in [
        "> ## Review round 2",
        "- ## Review round 2",
        "    ## Review round 2",
        "```\n## Review round 2\n```",
        "<search\n    ## Review round 2",
        "- <search\n  ## Review round 2",
        "<!-- a -->\n\n```\n## Review round 2\n```",
        "<b>a</b>\n\n~~~\n## Review round 2\n~~~",
        "So:\n\n```\n## Review round <n>\n\nReviewers: reviewer\n```\n\nA Vec<u8>.",
        "See:\n\n```\n## Review round 4\n```\n\n<details>\n\nx",
        "- a\n\n  ## Review round 2",
    ] {
        let comment = bot(&format!("{quoted}\n\nNo fields."));
        assert_eq!(
            check(&reviewed(vec![comment, bot(ROUND)])),
            Vec::<String>::new(),
            "{quoted}"
        );
    }
    let ruled = bot(&format!("a\n\n***\n\n{ROUND}"));
    assert_eq!(check(&reviewed(vec![ruled])), Vec::<String>::new());
    let before = old("Hot path: `send`\n\n## Review round 1\n\nNo fields.");
    assert_eq!(
        check(&reviewed(vec![before, bot(ROUND)])),
        Vec::<String>::new()
    );
}

#[test]
fn an_old_round_reads_no_hot_path_after_other_text_on_its_line() {
    // A `2.` item cannot interrupt a paragraph, so GitHub shows `2. Hot path: send`.
    let round =
        old("## Review round 1\n\nConfirmed a finding.\n\nSee\n2. Hot path: `send`");
    assert_eq!(
        check(&record(vec![round, bot(ROUND)])),
        Vec::<String>::new()
    );
}

#[test]
fn reads_an_old_round_with_html_or_a_bracket_as_before() {
    let fields =
        "Reviewers: reviewer, architecture, breaker\nRange: `a..b`\nFindings: 2";
    let html = old(&format!("## Review round 1\n\n{fields}\n\n<div>"));
    let hidden = old("<search\n## Review round 1\n\nHot path: `send`");
    let bracket = old(&format!("## Review round 1\n\n{fields}\n\na [^x\nb]"));
    for before in [html, hidden, bracket] {
        assert_eq!(
            check(&reviewed(vec![before, bot(ROUND)])),
            Vec::<String>::new()
        );
    }
}

#[test]
fn reads_a_line_break_in_an_image_text_as_github_shows_it() {
    // GitHub shows the image as one character on the line of `Reviewers:`, so the
    // `Findings:` line that GitHub shows is `Findings: 2`.
    for image in ["![a\nb](x)", "![a\\\nb](x)"] {
        let comment = second().replace("Findings: none", "Findings: 2").replace(
            "Reviewers: reviewer",
            &format!("Reviewers: reviewer, {image}Findings: none"),
        );
        assert_eq!(
            check(&reviewed(vec![bot(&comment)])),
            vec![
                "review round 2 has findings (2). Fix or answer them, then run \
                 another round."
            ],
            "{image}"
        );
    }
}

#[test]
fn an_old_round_reads_a_footnote_reference_as_github_shows_it() {
    // GitHub shows `Hot path: 1none` and `Hot path: 1`: no first word is `none`.
    for path in ["[^1]none\n\n[^1]: x", "[^none]\n\n[^none]: x"] {
        let round = old(&format!(
            "## Review round 1\n\nConfirmed a finding.\n\nHot path: {path}"
        ));
        assert_eq!(
            check(&record(vec![round, bot(ROUND)])),
            vec!["review round 1 names no performance, which this round requires."],
            "{path}"
        );
    }
}

#[test]
fn fails_end_lines_in_a_quote_or_a_list() {
    let end = "Deferred: none\nPublic surface: none\nHot path: none";
    for nested in [
        "> Deferred: none\n> Public surface: none\n> Hot path: none",
        "- Deferred: none\n  Public surface: none\n  Hot path: none",
    ] {
        let comment = ROUND.replace(end, nested);
        assert_ne!(comment, ROUND);
        assert_eq!(
            check(&record(vec![bot(&comment)])),
            vec![unended("Deferred")],
            "{nested}"
        );
    }
}

#[test]
fn an_old_round_that_defers_but_names_no_hot_path_needs_no_performance() {
    let round = ROUND.replace("Deferred: none", "Deferred: #12");
    assert_eq!(check(&reviewed(vec![old(&round)])), Vec::<String>::new());
}

/// A name line.
const NAME: &str = "**laptop.integrator-2** · integrator, #1211";

/// A summary line.
const SUMMARY: &str = "The fixes close each round 1 finding.";

/// `ROUND` with `head` before its round heading, posted at `created`.
fn headed_at(head: &str, created: &str) -> Comment {
    let (_, rest) = ROUND.split_once("## Review round").unwrap();
    Comment {
        created: created.to_string(),
        ..bot(&format!("{head}\n\n## Review round{rest}"))
    }
}

/// `ROUND` with `head` before its round heading, posted at the cutoff of the rating.
fn headed(head: &str) -> Comment {
    headed_at(head, RATING_CUTOFF)
}

/// The problem of round 3 that asks for `what`.
fn unrated(what: &str) -> Vec<String> {
    vec![format!(
        "review round 3 {what}. Write the round in the format of \
         .claude/skills/review/SKILL.md, \"Round comment\"."
    )]
}

#[test]
fn passes_a_round_that_starts_with_its_name_line_and_rating() {
    for head in [
        format!("{NAME}\nQuality: 8/10\n{SUMMARY}"),
        format!("{NAME}\n\nQuality: 8/10\n\n{SUMMARY}\nA second line."),
        format!("{NAME}\nQuality: 1/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 10/10\n{SUMMARY}"),
        format!("  {NAME}  \n  Quality:  8/10\n{SUMMARY}"),
        "__box1.builder-3__ · builder\nQuality: 5/10\nIt works.".to_string(),
    ] {
        let found = check(&reviewed(vec![headed(&head)]));
        assert_eq!(found, Vec::<String>::new(), "{head}");
    }
}

#[test]
fn fails_a_round_that_does_not_start_with_a_name_line() {
    let rating = format!("Quality: 8/10\n{SUMMARY}");
    for head in [
        String::new(),
        rating.clone(),
        format!("laptop.integrator-2 · integrator\n{rating}"),
        format!("**laptop.integrator-2** integrator\n{rating}"),
        format!("**laptop.integrator-2** ·\n{rating}"),
        format!("**laptop.integrator-2**\n{rating}"),
        format!("**laptop.integrator-2 · integrator**\n{rating}"),
        format!("**\u{200b}** · integrator\n{rating}"),
        format!("A note.\n{NAME}\n{rating}"),
        format!("A note.\n\n{NAME}\n{rating}"),
        format!("A note. {NAME}\n{rating}"),
        format!("```\n{NAME}\n{rating}\n```"),
        format!("    {NAME}\n    {rating}"),
        format!("> {NAME}\n> {rating}"),
        format!("- {NAME}\n  {rating}"),
        format!("- a\n\n{NAME}\n{rating}"),
        format!("# **laptop.integrator-2** notes\n\n{NAME}\n{rating}"),
        format!("**a** note · role\n{rating}"),
        format!("*laptop.integrator-2* · integrator\n{rating}"),
        format!("`laptop.integrator-2` · integrator\n{rating}"),
        format!("**![](x)** · integrator\n{rating}"),
        format!("**![](x) ![](y)** · integrator\n{rating}"),
        format!("**laptop.integrator-2** · ![](x)\n{rating}"),
        format!("**laptop.integrator-2** · ![](x) ![](y)\n{rating}"),
        format!(
            "laptop.integrator-2 · integrator\n{rating}\n\n**laptop.integrator-2** \
             wrote this round."
        ),
    ] {
        assert_eq!(
            check(&record(vec![headed(&head)])),
            unrated("does not start with a name line `**<session>** · <role>`"),
            "{head}"
        );
    }
}

#[test]
fn fails_a_round_with_no_rating_from_1_to_10_after_its_name_line() {
    for head in [
        NAME.to_string(),
        format!("{NAME}\n{SUMMARY}"),
        format!("{NAME}\n{SUMMARY}\nQuality: 8/10"),
        format!("{NAME}\nQuality: 0/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 11/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 8/9\n{SUMMARY}"),
        format!("{NAME}\nQuality: 8\n{SUMMARY}"),
        format!("{NAME}\nQuality: +8/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 08/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 8 / 10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 8/10, up from 6/10\n{SUMMARY}"),
        format!("{NAME}\nQuality: 8/10. {SUMMARY}"),
        format!("{NAME}\nquality: 8/10\n{SUMMARY}"),
        format!("{NAME}\nQuality 8/10\n{SUMMARY}"),
        format!("{NAME}\n\n- Quality: 8/10\n\n{SUMMARY}"),
    ] {
        assert_eq!(
            check(&record(vec![headed(&head)])),
            unrated(
                "has no line `Quality: <n>/10` with a rating from 1 to 10 after its \
                 name line"
            ),
            "{head}"
        );
    }
}

#[test]
fn fails_a_round_with_no_summary_line_after_its_rating() {
    for head in [
        format!("{NAME}\nQuality: 8/10"),
        format!("{NAME}\nQuality: 8/10\n\u{200b}"),
        format!("{NAME}\nQuality: 8/10\n\n- {SUMMARY}"),
        format!("{NAME}\nQuality: 8/10\n\n> {SUMMARY}"),
        format!("{NAME}\nQuality: 8/10\n![](x)"),
        format!("{NAME}\nQuality: 8/10\n![](x) ![](y)"),
    ] {
        assert_eq!(
            check(&record(vec![headed(&head)])),
            unrated("has no summary line after its `Quality:` line"),
            "{head}"
        );
    }
}

#[test]
fn reads_an_emoji_name_in_a_summary_line_as_text() {
    let head = format!("{NAME}\nQuality: 8/10\n:octocat:");
    assert_eq!(check(&reviewed(vec![headed(&head)])), Vec::<String>::new());
}

#[test]
fn a_round_posted_before_the_cutoff_of_the_rating_needs_no_name_line() {
    let rating = format!("Quality: 8/10\n{SUMMARY}");
    let before = headed_at(&rating, "2026-10-11T15:59:59Z");
    assert_eq!(check(&reviewed(vec![before])), Vec::<String>::new());
    for created in [
        RATING_CUTOFF,
        "2026-10-11T16:00:01Z",
        "2026-10-14T09:30:00Z",
    ] {
        assert_eq!(
            check(&reviewed(vec![headed_at(&rating, created)])),
            unrated("does not start with a name line `**<session>** · <role>`"),
            "{created}"
        );
    }
}

#[test]
fn fails_a_round_with_a_block_that_is_not_a_paragraph_between_its_three_lines() {
    let name = "has no line `Quality: <n>/10` with a rating from 1 to 10 after its \
                name line";
    let summary = "has no summary line after its `Quality:` line";
    for between in [
        "- The breaker found a defect.",
        "> a",
        "```\na\n```",
        "| a |\n|---|\n| b |",
        "## Notes",
        "---",
    ] {
        for (head, what) in [
            (
                format!("{NAME}\n\n{between}\n\nQuality: 8/10\n{SUMMARY}"),
                name,
            ),
            (
                format!("{NAME}\nQuality: 8/10\n\n{between}\n\n{SUMMARY}"),
                summary,
            ),
        ] {
            assert_eq!(check(&record(vec![headed(&head)])), unrated(what), "{head}");
        }
    }
}

#[test]
fn names_raw_html_before_the_name_line_of_a_rated_round_as_raw_html() {
    let head = format!("<details>\n\n{NAME}\nQuality: 8/10\n{SUMMARY}");
    assert_eq!(check(&record(vec![headed(&head)])), vec![raw("<details>")]);
}

#[test]
fn fails_each_round_with_no_name_line_and_names_it_before_a_missing_field() {
    let rating = format!("Quality: 8/10\n{SUMMARY}");
    let free = Comment {
        created: RATING_CUTOFF.to_string(),
        ..bot(&format!("{rating}\n\n## Review round 2\n\nNo field."))
    };
    let last = headed(&format!("{NAME}\n{rating}"));
    assert_eq!(
        check(&record(vec![free, last])),
        vec![
            "review round 2 does not start with a name line `**<session>** · <role>`. \
             Write the round in the format of .claude/skills/review/SKILL.md, \"Round \
             comment\"."
        ]
    );
}

#[test]
fn fails_a_round_that_github_starts_with_a_code_block_after_a_footnote() {
    let head = format!("[^1]: x\n \n    Junk\n\n{NAME}\nQuality: 8/10\n{SUMMARY}");
    assert_eq!(
        check(&record(vec![headed(&head)])),
        unrated("does not start with a name line `**<session>** · <role>`")
    );
}

#[test]
fn fails_a_rating_that_github_shows_with_a_backslash_after_it() {
    for end in ["\\ ", "\\\t", "\\ \t "] {
        let head = format!("{NAME}\nQuality: 8/10{end}\n{SUMMARY}");
        assert_eq!(
            check(&record(vec![headed(&head)])),
            unrated(
                "has no line `Quality: <n>/10` with a rating from 1 to 10 after its \
                 name line"
            ),
            "{end:?}"
        );
    }
}

#[test]
fn fails_a_round_that_github_ends_with_a_code_block_after_a_footnote() {
    let tail = ROUND.to_string() + "\n\n[^1]: x\n \n    Hot path: `send`";
    assert_eq!(check(&record(vec![bot(&tail)])), vec![unended("Deferred")]);
}

#[test]
fn reads_the_character_u_fffc_as_text_not_as_an_image() {
    for head in [
        format!("{NAME}\nQuality: 8/10\n\u{FFFC}"),
        format!("{NAME}\nQuality: 8/10\n&#xFFFC;"),
        format!("**\u{FFFC}** · integrator\nQuality: 8/10\n{SUMMARY}"),
        format!("**laptop.integrator-2** · \u{FFFC}\nQuality: 8/10\n{SUMMARY}"),
    ] {
        let found = check(&reviewed(vec![headed(&head)]));
        assert_eq!(found, Vec::<String>::new(), "{head:?}");
    }
}

/// `body` after a name line, a rating, and a summary.
fn rated(body: &str) -> String {
    format!("{NAME}\nQuality: 8/10\n{SUMMARY}\n\n{body}")
}

/// A rated comment by the bot, posted at the decision cutoff.
fn bound(body: &str) -> Comment {
    Comment {
        created: DECISION_CUTOFF.to_string(),
        ..bot(&rated(body))
    }
}

/// A round comment with no finding.
fn clean(number: u32, reviewers: &str, range: &str) -> String {
    format!(
        "## Review round {number}\n\nReviewers: {reviewers}\nRange: `{range}`\n\
         Findings: none{END}"
    )
}

/// The problem of round 2 with no `architecture`, whose range changes the decision
/// `x.md`.
const DECISION: &str = "review round 2 names no architecture, which this round \
                        requires. Run `architecture` on its range, or on the whole PR \
                        when the clone cannot read the range, then edit the round \
                        comment: add `architecture` to `Reviewers:` and add its \
                        findings. Its range changes the decision \
                        `docs/decisions/x.md`.";

/// The phrase of `DECISION` for what its range does.
const CHANGE: &str = "changes the decision `docs/decisions/x.md`";

/// What the check gives for `record` at the head of `repo`.
fn check_at(repo: &Repo, record: &Record) -> Vec<String> {
    problems_in(&repo.dir, record, &repo.head()).unwrap()
}

#[test]
fn a_later_round_whose_range_changes_a_decision_requires_architecture() {
    let (repo, start) = Repo::with_pr("review-decision");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    let range = format!("{start}..{end}");
    let rounds =
        |reviewers| record(vec![first(END), bound(&clean(2, reviewers, &range))]);
    assert_eq!(
        check_at(&repo, &rounds("reviewer, breaker")),
        vec![DECISION]
    );
    assert_eq!(
        check_at(&repo, &rounds("performance")),
        vec![
            "review round 2 names no reviewer, breaker, which this round requires.",
            DECISION
        ]
    );
    assert_eq!(
        check_at(&repo, &rounds("reviewer, architecture, breaker")),
        Vec::<String>::new()
    );
    repo.remove();
}

#[test]
fn a_later_round_that_adds_a_decision_in_another_case_requires_architecture() {
    let (repo, start) = Repo::with_pr("review-decision-case");
    let end = repo.commit("docs/Decisions/x.md", "a rule\n");
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &end]),
        "a.txt\nb.txt\ndocs/Decisions/x.md"
    );
    let round = bound(&clean(2, "reviewer, breaker", &format!("{start}..{end}")));
    assert_eq!(
        check_at(&repo, &record(vec![first(END), round])),
        vec![DECISION.replace("docs/decisions/x.md", "docs/Decisions/x.md")]
    );
    repo.remove();
}

#[test]
fn a_later_round_of_a_text_pr_whose_range_changes_a_decision_requires_architecture() {
    let (repo, start) = Repo::with_pr("review-decision-text");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    let range = format!("{start}..{end}");
    let rounds = |reviewers| Record {
        files: vec!["docs/decisions/x.md".to_string()],
        ..record(vec![
            bound(&clean(1, "reviewer", &range)),
            bound(&clean(2, reviewers, &range)),
        ])
    };
    assert_eq!(check_at(&repo, &rounds("reviewer")), vec![DECISION]);
    assert_eq!(
        check_at(&repo, &rounds("reviewer, architecture")),
        Vec::<String>::new()
    );
    repo.remove();
}

#[test]
fn a_second_round_comment_numbered_1_requires_architecture_too() {
    let (repo, start) = Repo::with_pr("review-decision-renumbered");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    let rounds = |reviewers| Record {
        files: vec!["docs/decisions/x.md".to_string()],
        ..record(vec![
            bound(&clean(1, "reviewer", &format!("a..{start}"))),
            bound(&clean(1, reviewers, &format!("{start}..{end}"))),
        ])
    };
    assert_eq!(
        check_at(&repo, &rounds("reviewer")),
        vec![DECISION.replace("round 2", "round 1")]
    );
    assert_eq!(
        check_at(&repo, &rounds("reviewer, architecture")),
        Vec::<String>::new()
    );
    repo.remove();
}

#[test]
fn a_later_round_that_is_not_the_last_requires_architecture_too() {
    let (repo, start) = Repo::with_pr("review-decision-earlier");
    let middle = repo.commit("docs/decisions/x.md", "a rule\n");
    let end = repo.commit("docs/x.md", "a doc\n");
    let record = record(vec![
        first(END),
        bound(&clean(
            2,
            "reviewer, breaker",
            &format!("{start}..{middle}"),
        )),
        bound(&clean(3, "reviewer, breaker", &format!("{middle}..{end}"))),
    ]);
    assert_eq!(check_at(&repo, &record), vec![DECISION]);
    repo.remove();
}

#[test]
fn a_later_round_posted_before_the_decision_cutoff_keeps_its_reviewers() {
    let (repo, start) = Repo::with_pr("review-decision-cutoff");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    let round = |created: &str| Comment {
        created: created.to_string(),
        ..bot(&rated(&clean(
            2,
            "reviewer, breaker",
            &format!("{start}..{end}"),
        )))
    };
    assert_eq!(
        check_at(
            &repo,
            &record(vec![first(END), round("2026-10-11T15:59:59Z")])
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        check_at(
            &repo,
            &record(vec![first(END), round("2026-10-11T16:00:01Z")])
        ),
        vec![DECISION]
    );
    repo.remove();
}

#[test]
fn a_first_round_comment_numbered_2_of_a_text_pr_needs_architecture_for_a_decision() {
    let check = |number| {
        let mut record = record(vec![bound(&clean(number, "reviewer", "a..b"))]);
        record.files = vec!["docs/decisions/x.md".to_string()];
        problems(
            &record,
            HEAD,
            &|_| Ok(Place::Head),
            &|_, _| Ok(None),
            &|_, _| Ok(Some(CHANGE.to_string())),
        )
    };
    assert_eq!(check(2), Ok(vec![DECISION.to_string()]));
    assert_eq!(check(1), Ok(Vec::new()));
}

#[test]
fn a_second_round_comment_numbered_1_of_a_code_pr_gets_one_problem() {
    let (repo, start) = Repo::with_pr("review-decision-renumbered-code");
    let end = repo.commit("docs/decisions/x.md", "a rule\n");
    let record = record(vec![
        bound(&clean(
            1,
            "reviewer, architecture, breaker",
            &format!("a..{start}"),
        )),
        bound(&clean(1, "reviewer, breaker", &format!("{start}..{end}"))),
    ]);
    assert_eq!(
        check_at(&repo, &record),
        vec![DECISION.replace("round 2", "round 1")]
    );
    repo.remove();
}

#[test]
fn a_later_round_whose_merge_moves_pr_text_into_a_decision_requires_architecture() {
    let (repo, _) = Repo::with_pr("review-decision-move");
    let (_, start, end) = repo.move_on_main("docs/drafts/x.md", "docs/decisions/x.md");
    let round = bound(&clean(2, "reviewer, breaker", &format!("{start}..{end}")));
    assert_eq!(
        check_at(&repo, &record(vec![first(END), round])),
        vec![DECISION.replace(
            CHANGE,
            "has `docs/drafts/x.md`, which the PR changes and the base moves into the \
             decision `docs/decisions/x.md`"
        )]
    );
    repo.remove();
}

#[test]
fn a_later_round_whose_range_names_no_commit_requires_architecture() {
    let (repo, end) = Repo::with_pr("review-decision-unnamed");
    let round = bound(&clean(2, "reviewer, breaker", &format!("abc1234..{end}")));
    assert_eq!(
        check_at(&repo, &record(vec![first(END), round])),
        vec![DECISION.replace(CHANGE, "has `abc1234`, which names no commit")]
    );
    repo.remove();
}

#[test]
fn a_last_round_that_ends_before_a_code_commit_fails_at_the_head() {
    let (repo, start) = Repo::with_pr("review-end-code");
    let end = repo.commit("docs/x.md", "a doc\n");
    let round = bound(&clean(2, "reviewer, breaker", &format!("{start}..{end}")));
    let record = record(vec![first(END), round]);
    assert_eq!(check_at(&repo, &record), Vec::<String>::new());
    let head = repo.commit("a.rs", "fn a() {}\n");
    assert_eq!(
        check_at(&repo, &record),
        vec![format!(
            "review round 2 ends at `{end}`, and the range from it to the head \
             `{head}` changes code at `a.rs:1`. A commit that changes code needs a \
             new round."
        )]
    );
    repo.remove();
}

#[test]
fn a_last_round_that_ends_off_the_chain_of_the_head_fails() {
    let (repo, start) = Repo::with_pr("review-end-off");
    let end = repo.commit("docs/x.md", "a doc\n");
    let round = bound(&clean(2, "reviewer, breaker", &format!("{start}..{end}")));
    repo.git(&["reset", "--quiet", "--hard", &start]);
    let head = repo.commit("docs/y.md", "another doc\n");
    assert_eq!(
        check_at(&repo, &record(vec![first(END), round])),
        vec![format!(
            "review round 2 ends at {end}, not at the head {head}. A commit after \
             the round needs a new round; only a clean merge of the base, or a \
             commit that changes no code after a round with a `Text fixes:` line, \
             does not."
        )]
    );
    repo.remove();
}

#[test]
fn a_last_round_that_skips_the_breaker_on_a_range_that_changes_code_fails() {
    let (repo, start) = Repo::with_pr("review-skip-code");
    let skip =
        "reviewer\nBreaker: skipped, the check counts no code change in the range";
    let text = repo.commit("docs/x.md", "a doc\n");
    let round = |end: &str| {
        let round = bound(&clean(2, skip, &format!("{start}..{end}")));
        record(vec![first(END), round])
    };
    assert_eq!(check_at(&repo, &round(&text)), Vec::<String>::new());
    let code = repo.commit("a.rs", "fn a() {}\n");
    assert_eq!(
        check_at(&repo, &round(&code)),
        vec!["review round 2 skips `breaker`, but its range changes code at `a.rs:1`."]
    );
    repo.remove();
}

#[test]
fn reads_the_decisions_only_of_a_later_round_with_no_architecture() {
    let failed = |_: &str, _: &str| Err("git diff: bad".to_string());
    let check = |comments| {
        problems(
            &record(comments),
            HEAD,
            &|_| Ok(Place::Head),
            &|_, _| Ok(None),
            &failed,
        )
    };
    let range = format!("a..{HEAD}");
    let one = bound(&clean(1, "reviewer, architecture, breaker", &range));
    assert_eq!(check(vec![one]), Ok(Vec::new()));
    let named = bound(&clean(2, "reviewer, architecture, breaker", &range));
    assert_eq!(check(vec![named]), Ok(Vec::new()));
    let earlier = bot(&clean(2, "reviewer, breaker", &range));
    assert_eq!(check(vec![opening(), earlier]), Ok(Vec::new()));
    let later = bound(&clean(2, "reviewer, breaker", &range));
    assert_eq!(
        check(vec![opening(), later]),
        Err("git diff: bad".to_string())
    );
}

/// A link to a comment.
const OK: &str =
    "https://github.com/synnaxlabs/foundation/issues/12#issuecomment-6105098668";

/// The end of each problem of a missing comment link.
const LINK_HELP: &str = "A link is the URL of a GitHub comment in the text of the \
                         line, at its start or after a space or `(`: a `[text](url)` \
                         link shows no URL.";

/// The problem of a round 3 that defers to `issue` with no OK.
fn no_ok(issue: &str) -> Vec<String> {
    vec![format!(
        "review round 3 defers to {issue} with no link to the OK of the architect \
         after it on its `Deferred:` line. Add the link, or the words `not in a risk \
         crate` after the issue when its work is in no risk crate. {LINK_HELP}"
    )]
}

/// The problem of a round 3 whose `Public surface:` line holds the word `owed`.
const OWED_PROBLEM: &str = "review round 3 has the word `owed` on its `Public \
                            surface:` line. Get each approval, then put its link in \
                            the line.";

/// The problem of a round 3 whose `Public surface:` line names items with no link.
fn no_link() -> Vec<String> {
    vec![format!(
        "review round 3 names items on its `Public surface:` line with no link to an \
         approval, apart from a link after `departs from`. {LINK_HELP}"
    )]
}

/// `ROUND`, rated, with the end lines `end` in place of its `Deferred:` and
/// `Public surface:` lines, posted at `created`.
fn ended_at(end: &str, created: &str) -> Comment {
    let lines = "Deferred: none\nPublic surface: none";
    Comment {
        created: created.to_string(),
        ..bot(&rated(&ROUND.replace(lines, end)))
    }
}

/// `ROUND`, rated, with the `Deferred:` value `deferred`, posted at the cutoff of the
/// links.
fn deferred(deferred: &str) -> Comment {
    let end = format!("Deferred: {deferred}\nPublic surface: none");
    ended_at(&end, LINK_CUTOFF)
}

/// `ROUND`, rated, with the `Public surface:` value `surface`, posted at the cutoff of
/// the links.
fn surfaced(surface: &str) -> Comment {
    let end = format!("Deferred: none\nPublic surface: {surface}");
    ended_at(&end, LINK_CUTOFF)
}

/// What the check gives for `comments` on a PR that also changes `file`.
fn check_with(file: &str, comments: Vec<Comment>) -> Vec<String> {
    let mut record = reviewed(comments);
    record.files.push(file.to_string());
    check(&record)
}

/// What the check gives for `comments` on a PR that changes the risk crate `raft`.
fn risk(comments: Vec<Comment>) -> Vec<String> {
    check_with("crates/raft/src/log.rs", comments)
}

#[test]
fn a_last_round_of_a_risk_crate_pr_links_an_ok_after_each_deferred_issue() {
    assert_eq!(risk(vec![deferred("#12")]), no_ok("#12"));
    assert_eq!(risk(vec![deferred("none")]), Vec::<String>::new());
    for linked in [
        format!("#12 ({OK})"),
        format!("#12 {OK}"),
        format!("#12, #13 (both: {OK})"),
        format!("#12 (the frame cut,\n{OK})"),
        format!("#12 <{OK}>"),
        format!("#12({OK})"),
        format!("#12 ({OK}), #13 ({OK})"),
    ] {
        assert_eq!(
            risk(vec![deferred(&linked)]),
            Vec::<String>::new(),
            "{linked}"
        );
    }
    for (line, issue) in [
        (format!("#12 ({OK}), #13"), "#13"),
        (format!("{OK} #12"), OK),
        (format!("#12 ({OK}), #13 (the cut), #14"), "#13"),
        (format!("#12 ({OK}), #13 ({OK}), #14"), "#14"),
        (format!("(#12), {OK}, PR#13,#14"), "#13"),
        (
            "#12 ([the OK](https://github.com/a/b/pull/1#issuecomment-2))".into(),
            "#12",
        ),
        ("the cut (#0012).".to_string(), "#0012"),
        ("#12,https://github.com/a/b/pull/1".to_string(), "#12"),
        (format!("{OK},#12 #13"), OK),
        ("none, #12 is fixed in this PR".to_string(), "#12"),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), no_ok(issue), "{line}");
    }
}

#[test]
fn a_comment_link_with_no_issue_before_it_is_a_deferred_issue_and_no_ok() {
    for line in [
        OK.to_string(),
        format!("none {OK}"),
        format!("{OK}, #12 not in a risk crate"),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), no_ok(OK), "{line}");
    }
    for line in [
        format!("{OK} ({OK})"),
        format!("{OK} not in a risk crate"),
        format!("#12 ({OK})"),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), Vec::<String>::new(), "{line}");
    }
}

#[test]
fn a_deferred_issue_is_a_number_sign_a_gh_name_or_the_url_of_an_issue() {
    let at = |rest: &str| format!("https://github.com/{rest}");
    for (line, issue) in [
        (format!("#12 <{OK}>,#13"), "#13".to_string()),
        (format!("#12({OK}),#13"), "#13".to_string()),
        (format!("#12 (<{OK}>);#13"), "#13".to_string()),
        (format!("#12 (`{OK}`),#13"), "#13".to_string()),
        (
            format!("#12 [{OK}](https://example.com),#13"),
            "#13".to_string(),
        ),
        (format!("#12 {OK}<#13"), "#13".to_string()),
        (format!("#12 ({OK}), see://#13"), "#13".to_string()),
        (
            format!("#12 ({OK}), www.github.com/a/b/issues/1#13"),
            "#13".to_string(),
        ),
        (format!("#12 ({OK}), GH-13"), "GH-13".to_string()),
        (format!("#12 ({OK}), (gh-013)."), "gh-013".to_string()),
        (format!("#12 ({OK}),GH-13"), "GH-13".to_string()),
        (format!("#12 ({OK}), éGH-13"), "GH-13".to_string()),
        ("GH-13".to_string(), "GH-13".to_string()),
        (at("a/b/issues/13"), at("a/b/issues/13")),
        (at("a/b/pull/13/files"), at("a/b/pull/13")),
        (
            format!(
                "the step of the merge order with #12 ({OK}): {}",
                at("a/b/issues/13")
            ),
            at("a/b/issues/13"),
        ),
        (
            format!("#12 ({OK}), {}", at("gh-1/b/issues/13")),
            at("gh-1/b/issues/13"),
        ),
        (
            format!("{} ({OK}), #14", at("a/b/issues/13#13")),
            "#14".to_string(),
        ),
        (
            format!("(https://x.io/?{})", at("a/b/pull/13#issuecomment-2")),
            at("a/b/pull/13"),
        ),
        (
            format!("#12 ({OK}), {}", at("gh-1/b/issues")),
            "gh-1".to_string(),
        ),
        (format!("#12 ({OK}), #13_"), "#13".to_string()),
        (format!("#12 ({OK}), #13abc"), "#13".to_string()),
        (format!("#12 ({OK}), `&#8203;`"), "#8203".to_string()),
        (format!("#12 ({OK}), @GH-13"), "GH-13".to_string()),
        (format!("#12{OK} not in a risk crate"), "#12".to_string()),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), no_ok(&issue), "{line}");
    }
    for line in [
        format!("{} ({OK})", at("gh-1/b/issues/13")),
        format!("{}#14 ({OK})", at("a/b/issues/13")),
        format!("{}, GH-14, #15: {OK}", at("a/b/pull/13")),
        format!("{}, {OK}", at("a/b/issues/13#14")),
        "the GH- and # marks, at https://github.com/a/b/issues/ and gh-x".to_string(),
        "https://gitlab.com/a/b/issues/13".to_string(),
        format!("#12 ({OK}), the high-13 line"),
        format!("#12 ({OK}), 1GH-13"),
        format!("#12 ({OK}), _GH-13"),
        format!("#12 ({OK}), x_GH-13"),
        "synnaxlabs/foundation/issues/13".to_string(),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), Vec::<String>::new(), "{line}");
    }
}

#[test]
fn a_gh_name_after_a_mark_or_an_ignorable_character_is_an_issue() {
    for name in [
        "**x**GH-13",
        "*x*GH-13",
        "`x`GH-13",
        "~~x~~GH-13",
        "[x](https://x.io)GH-13",
        "x&#8203;GH-13",
        "x\u{200B}GH-13",
        "x\u{AD}GH-13",
    ] {
        let line = format!("#12 ({OK}), {name}");
        assert_eq!(risk(vec![deferred(&line)]), no_ok("GH-13"), "{line:?}");
    }
    for name in ["**x**gh-13", "x\u{200B}gh-13"] {
        let line = format!("#12 ({OK}), {name}");
        assert_eq!(risk(vec![deferred(&line)]), no_ok("gh-13"), "{line:?}");
    }
    for name in ["xGH-13", "**xGH-13**"] {
        let line = format!("#12 ({OK}), {name}");
        assert_eq!(
            risk(vec![deferred(&line)]),
            Vec::<String>::new(),
            "{line:?}"
        );
    }
}

#[test]
fn an_issue_url_with_a_gh_repo_after_a_mark_or_an_ignorable_character_is_read() {
    let url = "https://github.com/a/gh-b/issues/13";
    let comment = "https://github.com/a/gh-b/pull/1#issuecomment-2";
    let hidden = "https://github.com/a/\u{200B}gh-b/pull/1#issuecomment-2";
    assert_eq!(risk(vec![deferred(hidden)]), no_ok(comment));
    for name in [
        "https://github.com/a/\u{200B}gh-b/issues/13",
        "https://github.com/a/\u{AD}gh-b/issues/13",
        "[https://github.com/a/**gh-b**/issues/13](https://x.io)",
    ] {
        let line = format!("#12 ({OK}), {name}");
        assert_eq!(risk(vec![deferred(&line)]), no_ok(url), "{line:?}");
    }
}

#[test]
fn a_comment_link_with_a_gh_name_after_an_ignorable_character_counts() {
    let url = "https://github.com/\u{200B}gh-1/b/pull/1#issuecomment-2";
    assert_eq!(
        risk(vec![deferred(&format!("#12 ({url})"))]),
        Vec::<String>::new()
    );
    let line = format!("`Share` ({url})");
    assert_eq!(
        check(&reviewed(vec![surfaced(&line)])),
        Vec::<String>::new()
    );
}

/// A limit of REVIEW CHECK: the check links a bare URL by the autolink rule of comrak.
/// GitHub also links text after `www.` with no domain with a dot, and a domain that
/// starts with a symbol such as `©`, and shows `&#40;` in it as written, but the check
/// reads it as `(`, so a comment link follows it. A read of the links from `body_html`
/// (#2473) gives each line a problem.
#[test]
fn a_bare_url_that_only_github_links_is_read_with_its_entities() {
    for url in ["www.~", "https://\u{A9}.y"] {
        let text = format!("{url}&#40;{OK})");
        let deferral = risk(vec![deferred(&format!("#12 {text}"))]);
        assert_eq!(deferral, Vec::<String>::new(), "{url}");
        let line = format!("x {text}");
        let surface = check(&reviewed(vec![surfaced(&line)]));
        assert_eq!(surface, Vec::<String>::new(), "{url}");
    }
}

/// GitHub links a bare URL up to the next space, and shows each mark in it as written.
#[test]
fn a_bare_url_shows_its_marks_as_written() {
    for url in [
        "https://github.com/*gh-1*/b/pull/1#issuecomment-2",
        "https://github.com/a/**b**/pull/1#issuecomment-2",
        "https://github.com/a/b/pull/1#issuecomment-*2*",
    ] {
        let line = format!("#12 ({url})");
        assert_eq!(risk(vec![deferred(&line)]), no_ok("#12"), "{line:?}");
        let line = format!("x ({url})");
        assert_eq!(
            check(&reviewed(vec![surfaced(&line)])),
            no_link(),
            "{line:?}"
        );
    }
    let line = format!("#12 ({OK}), https://github.com/a/&#8203;gh-b/issues/13");
    assert_eq!(risk(vec![deferred(&line)]), no_ok("#8203"));
}

#[test]
fn a_deferred_issue_that_only_the_target_of_a_link_names_is_not_read() {
    let target = "https://github.com/synnaxlabs/foundation/issues/13";
    let words = format!("[the frame cut]({target})");
    assert_eq!(risk(vec![deferred(&words)]), Vec::<String>::new());
    let number = format!("[#13]({target})");
    assert_eq!(risk(vec![deferred(&number)]), no_ok("#13"));
}

#[test]
fn no_deferred_issue_starts_in_a_comment_link() {
    let owner = "https://github.com/gh-1/b/pull/1#issuecomment-2";
    for line in [
        format!("#12 ({owner})"),
        format!("{owner} not in a risk crate"),
        format!("#12 ({owner} {OK})"),
    ] {
        assert_eq!(risk(vec![deferred(&line)]), Vec::<String>::new(), "{line}");
    }
    assert_eq!(risk(vec![deferred(owner)]), no_ok(owner));
    let after = format!("#12 ({owner}), GH-13");
    assert_eq!(risk(vec![deferred(&after)]), no_ok("GH-13"));
}

#[test]
fn a_deferred_issue_passes_with_the_mark_that_its_work_is_in_no_risk_crate() {
    for marked in [
        "#12 not in a risk crate".to_string(),
        "#12 (removes in other directories, `env`, not in a risk crate)".to_string(),
        "#12 (Not in a risk crate.)".to_string(),
        "#12 (the cut, not in a\nrisk crate), #13 (not in a risk crate)".to_string(),
        format!("#12 (not in a risk crate), #13 ({OK})"),
        format!("#12, #13 ({OK}), #14 (not in a risk crate)"),
    ] {
        assert_eq!(
            risk(vec![deferred(&marked)]),
            Vec::<String>::new(),
            "{marked}"
        );
    }
    for (line, issue) in [
        ("#12, #13 (not in a risk crate)", "#12"),
        ("#12 (not in a risk crate), #13", "#13"),
        ("not in a risk crate: #12", "#12"),
        ("#12 (not in a risk-crate)", "#12"),
    ] {
        assert_eq!(risk(vec![deferred(line)]), no_ok(issue), "{line}");
    }
}

#[test]
fn only_a_file_of_a_risk_crate_asks_a_deferred_issue_for_an_ok() {
    let check = |file: &str| check_with(file, vec![deferred("#12")]);
    for name in [
        "raft",
        "buffer",
        "delivery",
        "block",
        "ring",
        "codec",
        "wire",
        "home",
        "replica",
        "transport",
    ] {
        let file = format!("crates/{name}/Cargo.toml");
        assert_eq!(check(&file), no_ok("#12"), "{file}");
    }
    for file in ["crates/Raft/src/lib.rs", "Crates/RAFT/src/lib.rs"] {
        assert_eq!(check(file), no_ok("#12"), "{file}");
    }
    for file in [
        "crates/hub/src/lib.rs",
        "crates/rafts/src/lib.rs",
        "crates/raft-log/src/lib.rs",
        "crates/raft",
        "docs/crates/raft/x.md",
        "raft/src/lib.rs",
        "crates/hub/raft/x.rs",
    ] {
        assert_eq!(check(file), Vec::<String>::new(), "{file}");
    }
}

#[test]
fn a_pr_that_moves_a_file_changes_it_at_its_old_path_and_at_its_new_one() {
    use serde_json::json;
    let pull =
        [json!({ "head": { "ref": "a" }, "base": { "ref": "main" }, "labels": [] })];
    let comments = [rated(&opening().body), deferred("#12").body].map(|body| {
        json!({ "user": { "login": BOT }, "body": body, "created_at": LINK_CUTOFF })
    });
    let [old, new] = ["crates/raft/tests/it/log.rs", "crates/hub/tests/it/log.rs"];
    for (from, to) in [(old, new), (new, old)] {
        let moved = [json!({ "filename": to, "previous_filename": from })];
        let read = record_of(&pull, &moved, &comments).unwrap();
        assert_eq!(read.files, [from, to]);
        assert_eq!(check(&read), no_ok("#12"), "{from} to {to}");
    }
    let named = [json!({ "filename": new, "previous_filename": 1 })];
    assert_eq!(
        record_of(&pull, &named, &comments).unwrap_err(),
        "JSON has no string field `previous_filename`"
    );
}

#[test]
fn only_the_last_round_posted_at_the_link_cutoff_needs_its_links() {
    let before = "2026-10-12T11:59:59Z";
    let after = "2026-10-12T12:00:01Z";
    for (end, problems) in [
        ("Deferred: #12\nPublic surface: none", no_ok("#12")),
        (
            "Deferred: none\nPublic surface: `hub::Route`, approval owed",
            vec![OWED_PROBLEM.to_string()],
        ),
        ("Deferred: none\nPublic surface: `hub::Route`", no_link()),
    ] {
        assert_eq!(risk(vec![ended_at(end, before)]), Vec::<String>::new());
        assert_eq!(risk(vec![ended_at(end, LINK_CUTOFF)]), problems, "{end}");
        assert_eq!(risk(vec![ended_at(end, after)]), problems, "{end}");
        let earlier = ended_at(end, LINK_CUTOFF);
        let earlier = Comment {
            body: earlier
                .body
                .replace("## Review round 3", "## Review round 2"),
            ..earlier
        };
        let last = ended_at("Deferred: none\nPublic surface: none", LINK_CUTOFF);
        assert_eq!(risk(vec![earlier, last]), Vec::<String>::new(), "{end}");
    }
}

#[test]
fn only_the_last_round_comment_of_the_last_round_needs_its_surface_links() {
    let last = || ended_at("Deferred: none\nPublic surface: none", LINK_CUTOFF);
    for end in [
        "Deferred: none\nPublic surface: `hub::Route`, approval owed",
        "Deferred: none\nPublic surface: `hub::Route`",
    ] {
        let earlier = ended_at(end, LINK_CUTOFF);
        assert_eq!(risk(vec![earlier, last()]), Vec::<String>::new(), "{end}");
    }
}

#[test]
fn a_comment_link_is_the_url_of_a_comment_in_the_text_of_the_line() {
    let at = |rest: &str| format!("https://github.com/{rest}");
    for link in [
        at("a/b/pull/1#issuecomment-2"),
        at("a/b/issues/1#issuecomment-2"),
        at("a/b/pull/1#pullrequestreview-2"),
        at("a/b/pull/1#discussion_r2"),
        at("a-b/c.d_e/pull/10#issuecomment-2"),
        format!("`{}`", at("a/b/pull/1#issuecomment-2")),
        format!("({}).", at("a/b/pull/1#issuecomment-2")),
        format!("<{}>", at("a/b/pull/1#issuecomment-2")),
        format!("{} {}", at("a/b/pull/1"), at("a/b/pull/1#issuecomment-2")),
        format!("[{}](https://x.io)", at("a/b/pull/1#issuecomment-2")),
    ] {
        let line = format!("#12 {link}");
        assert_eq!(risk(vec![deferred(&line)]), Vec::<String>::new(), "{link}");
    }
    for link in [
        at("a/b/pull/1"),
        at("a/b/pull/1#issuecomment-"),
        at("a/b/pull/1#issuecomment-x"),
        at("a/b/pull/1#issue-2"),
        at("a/b/pull/#issuecomment-2"),
        at("a/b/pull/x#issuecomment-2"),
        at("a/b/commit/1#issuecomment-2"),
        at("a/b/pull/1/files#issuecomment-2"),
        at("a/pull/1#issuecomment-2"),
        at("/b/pull/1#issuecomment-2"),
        at("a//pull/1#issuecomment-2"),
        at("a$/b/pull/1#issuecomment-2"),
        at("a/b%20c/pull/1#issuecomment-2"),
        format!("(https://x.io/?{})", at("a/b/pull/1#issuecomment-2")),
        format!("(x{})", at("a/b/pull/1#issuecomment-2")),
        format!("see:{}", at("a/b/pull/1#issuecomment-2")),
        "http://github.com/a/b/pull/1#issuecomment-2".to_string(),
        "https://github.com.x.io/a/b/pull/1#issuecomment-2".to_string(),
        "github.com/a/b/pull/1#issuecomment-2".to_string(),
        format!("[the OK]({})", at("a/b/pull/1#issuecomment-2")),
        "#issuecomment-2".to_string(),
    ] {
        let line = format!("#12 {link}");
        assert_eq!(risk(vec![deferred(&line)]), no_ok("#12"), "{link}");
    }
}

#[test]
fn a_later_steps_line_is_no_part_of_the_deferred_or_the_public_surface_value() {
    let later = |deferred: &str, later: &str, surface: &str| {
        let end = format!(
            "Deferred: {deferred}\nLater steps: {later}\nPublic surface: {surface}"
        );
        risk(vec![ended_at(&end, LINK_CUTOFF)])
    };
    assert_eq!(later("none", "none", "none"), Vec::<String>::new());
    assert_eq!(
        later("none", "the rename, #99", "none"),
        Vec::<String>::new()
    );
    let linked = format!("the rename,\n#99 ({OK})");
    assert_eq!(later("#12", &linked, "none"), no_ok("#12"));
    assert_eq!(later("none", &linked, "`hub::Route`"), no_link());
    assert_eq!(
        later(
            "#12 (not in a risk crate)",
            "none",
            &format!("`hub::Route` ({OK})")
        ),
        Vec::<String>::new()
    );
    let after = format!(
        "Deferred: none\nPublic surface: `hub::Route`\nLater steps: #99 ({OK})"
    );
    assert_eq!(risk(vec![ended_at(&after, LINK_CUTOFF)]), no_link());
}

#[test]
fn a_later_steps_line_keeps_the_order_of_the_end_lines() {
    let ended = |end: &str| check(&reviewed(vec![bot(&ROUND.replace(END, end))]));
    let later = "Later steps: none";
    let [deferred, surface, hot] =
        ["Deferred: none", "Public surface: none", "Hot path: none"];
    for end in [
        format!("\n\n{deferred}\n{later}\n{surface}\n{hot}"),
        format!("\n\n{deferred}\n{surface}\n{later}\n{hot}"),
        format!("\n\n{deferred}\n{surface}\n{hot}\n{later}"),
        format!("\n\n{deferred}\n{later}\n{later}\n{surface}\n{hot}"),
    ] {
        assert_eq!(ended(&end), Vec::<String>::new(), "{end}");
    }
    let first = format!("\n\n{later}\n{deferred}\n{surface}\n{hot}");
    assert_eq!(ended(&first), vec![unended("Deferred")]);
    let only = format!("\n\n{deferred}\n{later}\n{hot}");
    assert_eq!(ended(&only), vec![unended("Public surface")]);
    let named = format!("\n\n{deferred}\n{surface}\nHot path:\nLater steps: none");
    assert_eq!(
        ended(&named),
        vec!["review round 3 names no performance, which this round requires."]
    );
}

/// The `Public surface:` value of the last round of PR 2372, of 2026-10-11T10:48:34Z.
const SURFACE_2372: &str = "\
    `crates/connector-opcua/src/server.rs:21-36` (`Server::start` takes `host` in \
    place of `url`, with its doc and `# Panics`), departs from \
    https://github.com/synnaxlabs/foundation/pull/2212#issuecomment-6099099141, \
    approved by \
    https://github.com/synnaxlabs/foundation/pull/2372#issuecomment-6108264821 at \
    `a9486b9df`; `docs/decisions/connectors/open62541-source.md:516-543`, departs \
    from https://github.com/synnaxlabs/foundation/pull/2212#issuecomment-6099099141, \
    approved by \
    https://github.com/synnaxlabs/foundation/pull/2372#issuecomment-6107958993 at \
    `c9a61bcaf`; the `Server::close` contract, approved by \
    https://github.com/synnaxlabs/foundation/pull/2372#issuecomment-6106770355 at \
    `d6d834286`; `crates/connector-opcua/src/bench.rs:148-149` (the `# Panics` of \
    `bench::Manager::scope`), approved by \
    https://github.com/synnaxlabs/foundation/pull/2372#issuecomment-6108214318 at \
    `6ac5b7127` (a9486b9df applies its own optional text fix)";

#[test]
fn a_last_round_links_an_approval_on_a_public_surface_line_that_names_items() {
    let check = |surface: &str| check(&reviewed(vec![surfaced(surface)]));
    for surface in [
        "none".to_string(),
        "none.".to_string(),
        "none, since no public item changes".to_string(),
        format!("`hub::Route`, approved by `laptop.architect` at `c77c67d7` ({OK})"),
        format!("`hub::Route` and `hub::Send`, both approved\n({OK})"),
        format!("`hub::Route` <{OK}>"),
        format!("{OK} approves `hub::Route`"),
        format!("`Share::owed` and `hub::owed()` ({OK})"),
        format!("`hub::Route`, departs from {OK}, approved in {OK}"),
        format!("`hub::Route` ({OK}), and `hub::Send` (departs from {OK})"),
        "none, since the doc of `hub::Route` departs from no ruling".to_string(),
        format!("`hub::Route`, approval owedé ({OK})"),
        format!("`hub::Route`, approval owed2 and 2owed ({OK})"),
        SURFACE_2372.to_string(),
    ] {
        assert_eq!(check(&surface), Vec::<String>::new(), "{surface}");
    }
    for surface in [
        "`hub::Route`, approval owed".to_string(),
        format!("`hub::Route` ({OK}); `hub::Send`, approval owed"),
        format!("`hub::Send`, approval\nowed; `hub::Route` ({OK})"),
        "`hub::Route`. Approval owed.".to_string(),
        "`hub::Route`, approval is owed".to_string(),
        "none, since the `hub::Route` change has approval owed in #12".to_string(),
        "none; `hub::Route`, approval owed".to_string(),
        format!("`hub::Route`, approval of `laptop.architect` owed ({OK})"),
        format!("`hub::Route`: approved in {OK}; approval at the head (OWED)."),
        format!("`hub::Route`, {OK}; the P1 judgment: `owed`"),
        format!("`hub::Route`, departs from {OK}, owed"),
        format!("`hub::Route` ({OK}); `hub::Send`, approval owed…"),
        format!("`hub::Route` ({OK}); `hub::Send`, approval “owed”"),
        format!("`hub::Route` ({OK}); `hub::Send`, approval owed![](x)"),
    ] {
        assert_eq!(check(&surface), vec![OWED_PROBLEM], "{surface}");
    }
    for surface in [
        "`hub::Route`".to_string(),
        "`hub::Route`, approved by `laptop.architect`".to_string(),
        "`hub::Route` ([approved](https://github.com/a/b/pull/1#issuecomment-2))"
            .to_string(),
        format!("`hub::Route`, departs from {OK}"),
        format!("`hub::Route`, Departs\nFrom the ruling ({OK})"),
        format!("`hub::Route` (departs from {OK}), `hub::Send` (departs from {OK})"),
        format!("`hub::Route` (x{OK})"),
        String::new(),
    ] {
        assert_eq!(check(&surface), no_link(), "{surface}");
    }
}

#[test]
fn a_last_round_gives_the_problem_of_each_end_line_after_its_other_problems() {
    let end = "Deferred: #12\nPublic surface: `hub::Route`";
    let round = ended_at(end, LINK_CUTOFF);
    let round = Comment {
        body: round
            .body
            .replace("Findings: none", "Findings: 1")
            .replace("## Review round 3", "## Review round 2"),
        ..round
    };
    let mut expected = vec![
        "review round 2 has findings (1). Fix or answer them, then run another round."
            .to_string(),
    ];
    expected.extend(no_ok("#12"));
    expected.extend(no_link());
    let expected: Vec<_> = expected
        .iter()
        .map(|problem| problem.replace("review round 3", "review round 2"))
        .collect();
    assert_eq!(risk(vec![round]), expected);
}

/// `ROUND` with `Findings: 2`, ended at `CODE`, a commit before the head after which
/// the range changes code.
fn capped() -> String {
    ROUND
        .replace("Findings: none", "Findings: 2")
        .replace("..c77c67d7`", &format!("..{CODE}`"))
}

#[test]
fn a_last_round_3_passes_with_findings_and_a_code_commit_after_it() {
    assert_eq!(check(&reviewed(vec![bot(&capped())])), Vec::<String>::new());
    let fourth = capped().replace("## Review round 3", "## Review round 4");
    assert_eq!(
        check(&reviewed(vec![bot(&capped()), bot(&fourth)])),
        Vec::<String>::new()
    );
}

/// `opening` with `Findings: 1`, numbered `number`.
fn found(number: u32) -> Comment {
    let round = opening();
    Comment {
        body: round
            .body
            .replace("Findings: none", "Findings: 1")
            .replace("## Review round 2", &format!("## Review round {number}")),
        ..round
    }
}

#[test]
fn a_last_round_2_fails_with_the_findings_of_an_earlier_round_2_comment() {
    let late = opening();
    let late = Comment {
        body: late.body.replace("`38cba24f..", "`c77c67d7.."),
        ..late
    };
    assert_eq!(
        check(&record(vec![found(2), late])),
        vec![
            "review round 2 has findings (1). Fix or answer them, then run another \
             round."
                .to_string()
        ]
    );
}

#[test]
fn a_last_round_2_passes_with_findings_only_in_round_1() {
    assert_eq!(
        check(&record(vec![found(1), opening()])),
        Vec::<String>::new()
    );
}

/// What the check gives for `round`, the last round, when the range from its end to
/// the head changes a decision ([`CHANGE`]).
fn decided(round: &str) -> Vec<String> {
    problems(
        &reviewed(vec![bot(round)]),
        HEAD,
        &|_| Ok(Place::Before),
        &|_, _| Ok(None),
        &|from, end| {
            assert_eq!([from, end], [CODE, HEAD]);
            Ok(Some(CHANGE.to_string()))
        },
    )
    .unwrap()
}

#[test]
fn a_last_round_3_fails_when_a_decision_changes_after_it_with_no_public_surface() {
    assert_eq!(
        decided(&capped()),
        vec![format!(
            "review round 3 ends at `{CODE}`, and the range from it to the head \
             `{HEAD}` {CHANGE}. Add the change to the `Public \
             surface:` line of the round, with its approval (REVIEW ROUNDS in \
             `docs/decisions/operations/review-rounds.md`)."
        )]
    );
}

#[test]
fn a_last_round_3_passes_a_decision_change_after_it_with_a_public_surface_item() {
    let named = capped().replace("Public surface: none", "Public surface: `x.md`");
    assert_eq!(decided(&named), Vec::<String>::new());
}

#[test]
fn a_last_round_2_with_findings_and_a_code_commit_after_it_fails() {
    let second = capped().replace("## Review round 3", "## Review round 2");
    assert_eq!(
        check(&reviewed(vec![bot(&second)])),
        vec![
            "review round 2 has findings (2). Fix or answer them, then run another \
             round."
                .to_string(),
            format!(
                "review round 2 ends at `{CODE}`, and the range from it to the head \
                 `{HEAD}` changes code at `a.rs:2`. A commit that changes code needs a \
                 new round."
            ),
        ]
    );
}

#[test]
fn a_last_round_3_with_findings_needs_the_ok_of_each_deferral_in_a_risk_crate() {
    let round = deferred("#12");
    let round = Comment {
        body: round.body.replace("Findings: none", "Findings: 2"),
        ..round
    };
    assert_eq!(risk(vec![round]), no_ok("#12"));
}

#[test]
fn a_last_round_3_with_findings_needs_each_reviewer_that_it_requires() {
    let round =
        later("reviewer, architecture").replace("Findings: none", "Findings: 2");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec!["review round 3 names no breaker, which this round requires.".to_string()]
    );
}

/// `ROUND` as round 2, below the round cap, so that its findings and its end count.
fn second() -> String {
    ROUND.replace("## Review round 3", "## Review round 2")
}

/// A second round 1 comment, as for a late report or a merge, with the range `from`
/// to `HEAD`.
fn repeated(from: &str) -> Comment {
    bot(&ROUND
        .replace("## Review round 3", "## Review round 1")
        .replace("38cba24f..c77c67d7", &format!("{from}..{HEAD}")))
}

#[test]
fn a_second_round_1_comment_is_a_later_round() {
    assert_eq!(
        check(&record(vec![settled(), repeated(HEAD)])),
        Vec::<String>::new()
    );
}

#[test]
fn a_second_round_1_comment_that_skips_breaker_on_code_fails() {
    assert_eq!(
        check(&record(vec![settled(), repeated(CODE)])),
        vec!["review round 1 skips `breaker`, but its range changes code at `a.rs:2`."]
    );
}

#[test]
fn a_first_round_comment_numbered_2_of_a_code_pr_requires_architecture() {
    let round =
        later("reviewer, breaker").replace("## Review round 3", "## Review round 2");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec!["review round 2 names no architecture, which this round requires."]
    );
}

/// `round` posted at the cutoff of the round numbers.
fn numbered(round: &str) -> Comment {
    Comment {
        created: ROUNDS_CUTOFF.to_string(),
        ..bot(&rated(round))
    }
}

#[test]
fn fails_a_round_comment_numbered_more_than_1_above_the_one_before_it() {
    assert_eq!(
        check(&record(vec![first(END), numbered(ROUND)])),
        vec![
            "review round 3 follows review round 1. Number each round comment 1 above \
             the round comment before it, or the same for a comment that only holds a \
             late report or only checks a merge of the base that is not clean, or a \
             merge and the commits that fix what it broke."
        ]
    );
}

#[test]
fn fails_a_round_comment_numbered_above_3_and_above_the_one_before_it() {
    let fourth = ROUND.replace("## Review round 3", "## Review round 4");
    assert_eq!(
        check(&reviewed(vec![numbered(ROUND), numbered(&fourth)])),
        vec![
            "review round 4 follows review round 3. After round 3, number each round \
             comment the same as the round comment before it."
        ]
    );
    assert_eq!(
        check(&reviewed(vec![numbered(ROUND), numbered(ROUND)])),
        Vec::<String>::new()
    );
    assert_eq!(
        check(&reviewed(vec![
            numbered(ROUND),
            bot(&fourth),
            numbered(&fourth)
        ])),
        Vec::<String>::new()
    );
}

#[test]
fn fails_a_first_round_comment_numbered_above_3() {
    let fourth = later("reviewer, architecture, breaker")
        .replace("## Review round 3", "## Review round 4");
    assert_eq!(
        check(&record(vec![numbered(&fourth)])),
        vec![
            "review round 4 is the first round comment on the PR. Number it 3 or less: \
             only a PR that replaces another before round 3 continues its count."
        ]
    );
    let third = later("reviewer, architecture, breaker");
    assert_eq!(check(&record(vec![numbered(&third)])), Vec::<String>::new());
}

#[test]
fn passes_round_comments_above_3_posted_before_the_cutoff_of_the_round_numbers() {
    let fourth = later("reviewer, architecture, breaker")
        .replace("## Review round 3", "## Review round 4");
    let fifth = fourth.replace("## Review round 4", "## Review round 5");
    assert_eq!(
        check(&record(vec![bot(&fourth), bot(&fifth)])),
        Vec::<String>::new()
    );
}

#[test]
fn passes_a_round_comment_numbered_1_above_or_the_same_as_the_one_before_it() {
    for round in [
        second(),
        ROUND.replace("## Review round 3", "## Review round 1"),
    ] {
        assert_eq!(
            check(&record(vec![settled(), numbered(&round)])),
            Vec::<String>::new(),
            "{round}"
        );
    }
}

#[test]
fn passes_a_round_comment_posted_before_the_cutoff_of_the_round_numbers() {
    assert_eq!(
        check(&record(vec![first(END), bot(ROUND)])),
        Vec::<String>::new()
    );
}

#[test]
fn a_merge_check_after_round_3_does_not_move_the_range_after_round_3() {
    let (repo, start) = Repo::with_pr("breaker-merge-check");
    let two = repo.commit("c.rs", "fn two() {}\n");
    let three = repo.commit("a.txt", "pr side\n");
    // The fix of a defect of round 3 changes a decision.
    let decided = repo.commit("docs/decisions/x.md", "a new rule\n");
    repo.git(&["switch", "--quiet", "main"]);
    let main = repo.commit("a.txt", "main side\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &main]);
    repo.git(&["switch", "--quiet", "pr"]);
    // A merge that resolves a conflict in `a.txt`, so it is not clean.
    repo.git(&["merge", "--quiet", "-X", "ours", "--no-edit", "origin/main"]);
    let head = repo.head();
    assert_eq!(
        Place::Unclean,
        history::History::new(&repo.dir, "main")
            .place(&decided, &head)
            .unwrap()
    );
    let round2 = clean(2, "reviewer, breaker", &format!("{start}..{two}"));
    let round3 = clean(3, "reviewer, breaker", &format!("{two}..{three}"))
        .replace("Findings: none", "Findings: 2");
    // The merge check: its range starts at the first parent of the merge.
    let check3 = clean(3, "reviewer, breaker", &format!("{decided}..{head}"));
    let record = record(vec![
        first(END),
        numbered(&round2),
        numbered(&round3),
        numbered(&check3),
    ]);
    assert_eq!(
        check_at(&repo, &record),
        vec![format!(
            "review round 3 ends at `{three}`, and the range from it to the head \
             `{head}` changes the decision `docs/decisions/x.md`. Add the change to \
             the `Public surface:` line of the round, with its approval (REVIEW \
             ROUNDS in `docs/decisions/operations/review-rounds.md`)."
        )]
    );
    repo.remove();
}

#[test]
fn a_merge_check_after_round_3_does_not_hide_a_deferral_of_round_3_with_no_ok() {
    let check = ended_at("Deferred: none\nPublic surface: none", LINK_CUTOFF);
    assert_eq!(risk(vec![deferred("#12"), check]), no_ok("#12"));
}

#[test]
fn a_deferral_of_the_last_round_needs_its_ok_on_one_comment_of_the_round() {
    let linked = || deferred(&format!("#12 ({OK})"));
    assert_eq!(risk(vec![deferred("#12"), linked()]), Vec::<String>::new());
    assert_eq!(risk(vec![linked(), deferred("#12")]), Vec::<String>::new());
    let other = deferred(&format!("#13 ({OK})"));
    assert_eq!(risk(vec![deferred("#12"), other]), no_ok("#12"));
}

#[test]
fn a_first_round_comment_numbered_2_names_a_missing_architecture_once() {
    let record = record(vec![bound(&clean(2, "reviewer, breaker", "a..b"))]);
    assert_eq!(
        problems(
            &record,
            HEAD,
            &|_| Ok(Place::Head),
            &|_, _| Ok(None),
            &|_, _| Ok(Some(CHANGE.to_string()))
        ),
        Ok(vec![
            "review round 2 names no architecture, which this round requires."
                .to_string()
        ])
    );
}

#[test]
fn a_first_round_comment_numbered_2_may_not_skip_breaker() {
    let round = ROUND
        .replace("## Review round 3", "## Review round 2")
        .replace("Reviewers: reviewer", "Reviewers: reviewer, architecture");
    assert_eq!(
        check(&record(vec![bot(&round)])),
        vec!["review round 2 names no breaker, which this round requires.".to_string()]
    );
}

#[test]
fn a_rebase_after_round_3_fails_after_a_round_3_comment_at_the_head() {
    let (repo, start) = Repo::with_pr("breaker-rebase-after-3");
    let two = repo.commit("c.rs", "fn two() {}\n");
    let three = repo.commit("c.rs", "fn three() {}\n");
    repo.git(&["switch", "--quiet", "main"]);
    let main = repo.commit("m.txt", "main\n");
    repo.git(&["update-ref", "refs/remotes/origin/main", &main]);
    repo.git(&["switch", "--quiet", "pr"]);
    // A rebase after round 3 moves the end of round 3 off the head's chain.
    repo.git(&["rebase", "--quiet", "main"]);
    let head = repo.head();
    assert_eq!(
        Place::Off,
        history::History::new(&repo.dir, "main")
            .place(&three, &head)
            .unwrap()
    );
    let round2 = clean(2, "reviewer, breaker", &format!("{start}..{two}"));
    let round3 = clean(3, "reviewer, breaker", &format!("{two}..{three}"));
    let check3 = clean(3, "reviewer, breaker", &format!("{main}..{head}"));
    let record = record(vec![
        first(END),
        numbered(&round2),
        numbered(&round3),
        numbered(&check3),
    ]);
    assert_eq!(
        check_at(&repo, &record),
        vec![format!(
            "review round 3 ends at {three}, which is not a commit on the first-parent \
             chain of the head {head}, or a merge of a branch other than the base \
             follows it. Correct the range of the round comment when it names the \
             wrong commit. Else bring the reviewed commits back onto that chain, with \
             only merges of the base after them."
        )]
    );
    repo.remove();
}

#[test]
fn a_round_3_whose_first_end_names_no_commit_fails_after_a_round_3_at_the_head() {
    let (repo, start) = Repo::with_pr("breaker-unnamed-end-after-3");
    let two = repo.commit("c.rs", "fn two() {}\n");
    let head = repo.commit("c.rs", "fn three() {}\n");
    let surface = format!("Public surface: `hub::Route`, approved at {OK}");
    let round2 = clean(2, "reviewer, breaker", &format!("{start}..{two}"));
    let round3 = clean(
        3,
        "reviewer, architecture, breaker",
        &format!("{two}..0badc0de"),
    );
    let check3 = clean(3, "reviewer, breaker", &format!("{two}..{head}"))
        .replace("Public surface: none", &surface);
    let record = record(vec![
        first(END),
        numbered(&round2),
        numbered(&round3),
        numbered(&check3),
    ]);
    assert_eq!(
        check_at(&repo, &record),
        vec![format!(
            "review round 3 ends at 0badc0de, which is not a commit on the first-parent \
             chain of the head {head}, or a merge of a branch other than the base \
             follows it. Correct the range of the round comment when it names the \
             wrong commit. Else bring the reviewed commits back onto that chain, with \
             only merges of the base after them."
        )]
    );
    repo.remove();
}
