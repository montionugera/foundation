//! The review check: the review of a PR is done at its head. The round comment format
//! it parses is in `.claude/skills/review/SKILL.md`, "Round comment".

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::Path;
use std::process::ExitCode;

use comrak::nodes::{AstNode, NodeHeading, NodeValue, Sourcepos};
use comrak::{Arena, Options, parse_document};
use serde_json::Value;

use crate::{field, gh};

mod history;

use history::Place;
#[cfg(test)]
pub(crate) use history::tests::Repo;

/// The first change of one kind in a range `<from>..<end>`, as a phrase, or `None`.
type Change<'a> = &'a dyn Fn(&str, &str) -> Result<Option<String>, String>;

/// The account that posts each round comment and each director verdict. Comments by
/// other accounts never count.
const BOT: &str = "synnax-foundation-factory[bot]";

/// The end of each problem that asks for the round comment format.
const FORMAT: &str =
    "in the format of .claude/skills/review/SKILL.md, \"Round comment\".";

/// What in a line of a comment can hide text on GitHub, or keeps the check from
/// reading it.
#[derive(Clone, Copy, Debug)]
enum Cause {
    /// Raw HTML.
    Raw,
    /// `[^` in a span of text before the last line of its paragraph: GitHub reads a `]`
    /// on a later line as the end of a footnote reference.
    Bracket,
    /// A link or an image with a line break after its text, or a link reference
    /// definition, in a paragraph: comrak then places its text in the wrong lines
    /// ([`misplaced`]).
    Misplaced,
    /// A footnote definition inside another one ([`nested`]).
    Nested,
}

impl Cause {
    /// Whether the check cannot read the text, so that the cause fails also an old
    /// round, which is otherwise taken as written.
    fn unreadable(self) -> bool {
        match self {
            Cause::Raw | Cause::Bracket => false,
            Cause::Misplaced | Cause::Nested => true,
        }
    }

    /// The problem of round `number` with this cause in `line`.
    fn problem(self, number: u32, line: &str) -> String {
        let (cause, remedy) = self.text(line);
        format!("review round {number} has {cause}. {remedy}, {FORMAT}")
    }

    /// What this cause in `line` is, and the remedy.
    fn text(self, line: &str) -> (String, &'static str) {
        match self {
            Cause::Raw => (
                format!(
                    "raw HTML, which can hide text on GitHub, in the line `{line}`"
                ),
                "Put the line in a code span",
            ),
            Cause::Bracket => (
                format!(
                    "`[^` before the last line of a paragraph, which can hide text on \
                     GitHub, in the line `{line}`"
                ),
                "Put the line in a code span",
            ),
            Cause::Misplaced => (
                format!(
                    "a link or an image with a line break after its text, or a link \
                     reference definition, in the paragraph that starts with the line \
                     `{line}`, which the check cannot read"
                ),
                "Write each link and image on one line, and put a blank line after \
                 each link reference definition",
            ),
            Cause::Nested => (
                format!(
                    "a footnote inside a footnote, which the check cannot read as \
                     GitHub shows it, in the line `{line}`"
                ),
                "Move it out of the other footnote",
            ),
        }
    }
}

/// The names of the lines that end each round comment, in order.
const END: [&str; 3] = ["Deferred", "Public surface", "Hot path"];

/// The name of a line that the end lines may hold. The check reads no value of it.
const LATER: &str = "Later steps";

/// The name of the line that lists the text fixes of a round.
const FIXES: &str = "Text fixes";

/// A round comment posted before this UTC time keeps the check from before the end
/// lines: it needs no end lines, and an earlier free-form one passes. It still needs
/// `performance` when a `Hot path:` line names a function.
const CUTOFF: &str = "2026-10-08T03:00:00Z";

/// A round comment posted before this UTC time needs no name line, rating, or summary
/// line before its round heading, so that the rule does not fail the rounds that came
/// before it.
const RATING_CUTOFF: &str = "2026-10-11T16:00:00Z";

/// A later round posted before this UTC time needs no `architecture` for a range that
/// changes a decision, so that the rule does not fail the rounds that came before it.
const DECISION_CUTOFF: &str = "2026-10-11T16:00:00Z";

/// The last round, when posted before this UTC time, needs no comment link on its
/// `Deferred:` and `Public surface:` lines, so that the rule does not fail the rounds
/// that came before it.
const LINK_CUTOFF: &str = "2026-10-12T12:00:00Z";

/// The crates in which a deferral needs the OK of the architect. It must match the
/// risk crates of ENGINEERS in `docs/decisions/operations/engineers.md`.
const RISK: [&str; 10] = [
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
];

/// The words on a `Deferred:` line that mark the issue before them as work in no risk
/// crate.
const UNRISKY: &str = "not in a risk crate";

/// The word on a `Public surface:` line of an item with no approval.
const OWED: &str = "owed";

/// The words on a `Public surface:` line before the link to the comment that an item
/// departs from, which is no approval.
const DEPARTS: &str = "departs from";

/// The end of each problem of a missing comment link.
const LINK: &str = "A link is the URL of a GitHub comment in the text of the line, at \
                    its start or after a space or `(`: a `[text](url)` link shows no \
                    URL.";

/// The record of a PR that the check reads.
#[derive(Debug)]
struct Record {
    branch: String,
    /// The branch the PR merges into.
    base: String,
    labels: Vec<String>,
    files: Vec<String>,
    comments: Vec<Comment>,
    /// Each issue or PR that a merge condition of the body names ([`conditions`]) and
    /// that is open, in body order, or the problem of a body that the check cannot read.
    waits: Result<Vec<u64>, String>,
}

#[derive(Debug)]
struct Comment {
    author: String,
    body: String,
    /// When the comment was posted, a UTC time (`utc`), so text order is time order.
    created: String,
}

/// A parsed `## Review round <n>` comment.
#[derive(Debug, PartialEq, Eq)]
struct Round {
    number: u32,
    reviewers: BTreeSet<String>,
    /// The round has the line `Breaker: skipped`: a later round may skip `breaker`
    /// when its range changes only comment and blank lines of `.rs` files.
    breakerless: bool,
    from: String,
    end: String,
    findings: u32,
    /// The comment has a line `Text fixes:` that is not `Text fixes: none`, so commits
    /// that apply its items may follow the round.
    fixed: bool,
    /// `None` for an old round, which has no end lines: [`Parsed`] checks its hot path.
    ends: Option<Ends>,
    /// When the comment was posted ([`Comment::created`]). A rule that starts at a
    /// cutoff compares it with its own.
    posted: String,
}

/// The end lines of a round comment.
#[derive(Debug, PartialEq, Eq)]
struct Ends {
    /// The value of the `Deferred:` line.
    deferred: String,
    /// The place in `deferred` of each [`BREAK`] that [`ends`] took out of it.
    breaks: Vec<usize>,
    /// The value of the `Public surface:` line.
    surface: String,
    /// The `Hot path:` line names a function, so the round requires `performance`.
    hot: bool,
}

/// A round comment that does not parse.
#[derive(Debug)]
struct Malformed {
    /// Its problem counts also when the round is not the last: the round is not old,
    /// or it has a `Reviewers:`, `Range:`, or `Findings:` line, so it is not
    /// free-form, or the check cannot read it ([`Shown::hiding`]).
    binding: bool,
    problem: String,
}

/// A comment with a round heading ([`Shown::number`]).
#[derive(Debug)]
struct Parsed {
    round: Result<Round, Malformed>,
    /// The problem of an old round that names a hot path ([`named`]) and does not
    /// name `performance`: in its `Reviewers:` field when it parses, else in a
    /// `Reviewers:` line ([`performer`]).
    performance: Option<String>,
}

/// Checks that the review of PR `pr` is done at commit `head`: its last round comment
/// ends at `head` or at a commit on its first-parent chain after which each merge is
/// a clean merge of the base and the range to `head` changes no code, finds nothing,
/// and names each required reviewer, and, for a red-team PR, the director approved
/// `head`. Reads the PR with `api` and the history of the repository at `root` with
/// `git`, and prints each problem.
///
/// Exits 0 when the review is done, 1 when it is not, and 2 when `api` or `git` fails
/// or the PR record lacks a field.
pub(crate) fn run(root: &Path, pr: &str, head: &str, api: gh::Read<'_>) -> ExitCode {
    exit(fetch(pr, api).and_then(|record| problems_in(root, &record, head)))
}

/// The problems with the review in `record` at `head` ([`problems`]), read with the
/// history of the repository at `root`.
fn problems_in(
    root: &Path,
    record: &Record,
    head: &str,
) -> Result<Vec<String>, String> {
    let history = history::History::new(root, &record.base);
    problems(
        record,
        head,
        &|end| history.place(end, head),
        &|from, end| history.code_change(from, end),
        &|from, end| history.decision_change(from, end),
    )
}

/// Prints each problem or the failure in `found`, and gives the exit code of `run`.
pub(crate) fn exit(found: Result<Vec<String>, String>) -> ExitCode {
    match found {
        Ok(problems) if problems.is_empty() => ExitCode::SUCCESS,
        Ok(problems) => {
            for problem in problems {
                eprintln!("error: {problem}\n");
            }
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// The problems with the review in `record` at `head`: each round must name the
/// reviewers it requires, and the last round must find nothing and end at `head`, or
/// at a commit on its first-parent chain after which each merge is a clean merge of
/// the base and the range to `head` changes no code, with a `Text fixes:` line when a
/// commit that is not a merge follows it. `place` gives where a commit (a SHA or its
/// prefix) is from `head` (`History::place`). A later round may skip `breaker` when
/// the range of the last round changes no code; an earlier round's skip is taken as
/// written. `code_change` gives the first code change in a range, with a merge of the
/// base read by its resolution, as a phrase, or `None` (`History::code_change`). A
/// later round posted at or after `DECISION_CUTOFF` must name `architecture` when
/// `decision_change` gives a phrase for its range (`History::decision_change`). For
/// that rule, a round comment after the first is a later round, whatever its number,
/// and so is a first round comment whose number is above 1. The last round, when
/// posted at or after `LINK_CUTOFF`, must link the OK of each deferral of a PR that
/// changes a risk crate, and an approval of its public surface ([`unlinked`]).
fn problems(
    record: &Record,
    head: &str,
    place: &dyn Fn(&str) -> Result<Place, String>,
    code_change: Change<'_>,
    decision_change: Change<'_>,
) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    let rounds: Vec<_> = record
        .comments
        .iter()
        .filter(|c| c.author == BOT)
        .filter_map(round)
        .collect();
    if rounds.is_empty() {
        problems.push(format!(
            "no review round comment by {BOT}. Run `/review` and post each round \
             {FORMAT}"
        ));
    }
    for (i, parsed) in rounds.iter().enumerate() {
        let last = i + 1 == rounds.len();
        match &parsed.round {
            Ok(round) => {
                let later = i > 0 || round.number > 1;
                problems.extend(unnamed(
                    round,
                    &record.files,
                    last,
                    code_change,
                    (later && round.posted.as_str() >= DECISION_CUTOFF)
                        .then_some(decision_change),
                )?);
            }
            Err(e) => {
                if last || e.binding {
                    problems.push(e.problem.clone());
                }
            }
        }
        problems.extend(parsed.performance.clone());
    }
    if let Some(Parsed {
        round: Ok(round), ..
    }) = rounds.last()
    {
        if round.findings > 0 {
            problems.push(format!(
                "review round {} has findings ({}). Fix or answer them, then run \
                 another round.",
                round.number, round.findings
            ));
        }
        problems.extend(unended(round, head, place(&round.end)?, code_change)?);
        problems.extend(unlinked(round, &record.files));
    }
    match &record.waits {
        Ok(waits) => problems.extend(waits.iter().map(|n| {
            format!(
                "the PR body has `Merges after #{n}`, and #{n} is open. Run `review` \
                 again when it merges or closes."
            )
        })),
        Err(problem) => problems.push(problem.clone()),
    }
    if record.branch.starts_with("red-team/")
        && record.labels.iter().any(|l| l == "oracle")
    {
        problems.extend(approval(record, head));
    }
    Ok(problems)
}

/// The problem with the end of the last round, `round`, at `head`, where `place` is
/// where its end is from `head`, or `None`.
fn unended(
    round: &Round,
    head: &str,
    place: Place,
    code_change: Change<'_>,
) -> Result<Option<String>, String> {
    let (number, end) = (round.number, &round.end);
    if place == Place::Off {
        return Ok(Some(format!(
            "review round {number} ends at {end}, not at the head {head}. A commit \
             after the round needs a new round; only a clean merge of the base, or a \
             commit that changes no code after a round with a `Text fixes:` line, \
             does not."
        )));
    }
    if let Some(change) = code_change(end, head)? {
        return Ok(Some(format!(
            "review round {number} ends at `{end}`, and the range from it to the head \
             `{head}` {change}. A commit that changes code needs a new round."
        )));
    }
    Ok((place == Place::Before && !round.fixed).then(|| {
        format!(
            "review round {number} ends at `{end}`, not at the head `{head}`, and has \
             no `Text fixes:` line."
        )
    }))
}

/// The problems with the links on the end lines of `round`, the last round of a PR
/// that changes `files`: the first issue of its `Deferred:` line with no OK
/// ([`unokayed`]) when a file is in a risk crate ([`RISK`]), and what its
/// `Public surface:` line lacks ([`unapproved`]). None for a round posted before
/// [`LINK_CUTOFF`].
fn unlinked(round: &Round, files: &[String]) -> Vec<String> {
    let bound = round.posted.as_str() >= LINK_CUTOFF;
    let Some(ends) = round.ends.as_ref().filter(|_| bound) else {
        return Vec::new();
    };
    let number = round.number;
    let risky = files.iter().any(|file| {
        let file = file.to_ascii_lowercase();
        let name = file.strip_prefix("crates/").and_then(|f| f.split_once('/'));
        name.is_some_and(|(name, _)| RISK.contains(&name))
    });
    let problem = unokayed(&ends.deferred, &ends.breaks)
        .filter(|_| risky)
        .map(|issue| {
            format!(
                "review round {number} defers to {issue} with no link to the OK of the \
                 architect after it on its `Deferred:` line. Add the link, or the \
                 words `{UNRISKY}` after the issue when its work is in no risk crate. \
                 {LINK}"
            )
        });
    problem
        .into_iter()
        .chain(unapproved(number, &ends.surface))
        .collect()
}

/// The problems with the reviewers that `round` names for a PR that changes `files`.
/// A later round may skip `breaker` when its range changes no code, which only the
/// `last` round checks with `code_change`: a rebase can drop the range of an earlier
/// round from the clone, but the last round's end must be on the head's chain. A
/// round with a `decision_change` requires `architecture` too when that gives a phrase
/// for its range. When `required` does not already ask for `architecture`, its problem
/// gives the remedy, then the phrase.
fn unnamed(
    round: &Round,
    files: &[String],
    last: bool,
    code_change: Change<'_>,
    decision_change: Option<Change<'_>>,
) -> Result<Vec<String>, String> {
    let mut problems = Vec::new();
    let mut missing: Vec<&str> = required(round, files)
        .into_iter()
        .filter(|name| !round.reviewers.contains(*name))
        .collect();
    if round.number > 1 && round.breakerless && missing.contains(&"breaker") {
        missing.retain(|name| *name != "breaker");
        if last && let Some(change) = code_change(&round.from, &round.end)? {
            problems.push(format!(
                "review round {} skips `breaker`, but its range {change}.",
                round.number
            ));
        }
    }
    if !missing.is_empty() {
        problems.push(format!(
            "review round {} names no {}, which this round requires.",
            round.number,
            missing.join(", ")
        ));
    }
    if !round.reviewers.contains("architecture")
        && !missing.contains(&"architecture")
        && let Some(decision_change) = decision_change
        && let Some(change) = decision_change(&round.from, &round.end)?
    {
        problems.push(format!(
            "review round {} names no architecture, which this round requires. Run \
             `architecture` on its range, or on the whole PR when the clone cannot \
             read the range, then edit the round comment: add `architecture` to \
             `Reviewers:` and add its findings. Its range {change}.",
            round.number
        ));
    }
    Ok(problems)
}

/// The reviewers that `round` must name for a PR that changes `files`, by REVIEW
/// TIERS in `docs/decisions/operations/review-tiers.md`: on round 1, `reviewer`, plus
/// `architecture` and `breaker` for a code PR; on a later round, `reviewer`, plus
/// `breaker` for a code PR; and `performance` when the round names a hot path.
fn required(round: &Round, files: &[String]) -> Vec<&'static str> {
    let code = files.iter().any(|f| tiered(f));
    let mut required = vec!["reviewer"];
    if code && round.number <= 1 {
        required.push("architecture");
    }
    if code {
        required.push("breaker");
    }
    if round.ends.as_ref().is_some_and(|ends| ends.hot) {
        required.push("performance");
    }
    required
}

/// Whether a change to `path` makes a code PR by REVIEW TIERS: a `.rs` file, a
/// `Cargo.toml`, or a `Cargo.lock`.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "git paths match case"
)]
fn tiered(path: &str) -> bool {
    Path::new(path)
        .file_name()
        .is_some_and(|n| n == "Cargo.toml" || n == "Cargo.lock")
        || path.ends_with(".rs")
}

/// A problem when no director verdict by the bot has the line
/// ``Director: approved at `<sha>` `` for `head`. A later push needs a new approval, so
/// the SHA must be `head` or a prefix of it of 7 or more digits.
fn approval(record: &Record, head: &str) -> Option<String> {
    let approved = record
        .comments
        .iter()
        .filter(|c| c.author == BOT)
        .flat_map(|c| lines(&c.body))
        .filter_map(|l| l.trim().strip_prefix("Director: approved at "))
        .map(|sha| sha.trim_matches(['`', '.']))
        .any(|sha| sha.len() >= 7 && head.starts_with(sha));
    (!approved).then(|| {
        format!(
            "a red-team `oracle` PR needs the director's verdict with the line \
             \"Director: approved at `<sha>`\" for the head {head}."
        )
    })
}

/// Reports whether a `Reviewers:` line in `text` ([`Shown::text`]) names
/// `performance`.
fn performer(text: &[Vec<String>]) -> bool {
    text.iter()
        .flatten()
        .filter_map(|l| l.strip_prefix("Reviewers: "))
        .any(|r| listed(r).contains("performance"))
}

/// Whether a top-level paragraph of `shown` after its round heading, other than its end
/// lines, has a line `Text fixes:` with a value ([`stated`]).
fn fixed(shown: &Shown<'_>) -> bool {
    let blocks = shown.before_end().iter();
    states(blocks.map(|block| shown.paragraph(Some(block))), FIXES)
}

/// The reviewers that the value of a `Reviewers:` line lists.
fn listed(reviewers: &str) -> BTreeSet<String> {
    reviewers
        .split(',')
        .map(|r| r.trim().trim_matches('`').to_string())
        .collect()
}

/// Parses `comment` as a round comment. `None` when it has no round heading
/// ([`Shown::number`]). The fields are the first block after the heading, so the
/// findings text cannot set them. The last block is the end lines ([`END`]), unless the
/// comment is old, posted before [`CUTOFF`]. A comment posted at or after
/// [`RATING_CUTOFF`] starts with its name line, its rating, and its summary line
/// ([`lack`]).
fn round(comment: &Comment) -> Option<Parsed> {
    let old = comment.created.as_str() < CUTOFF;
    let rated = comment.created.as_str() >= RATING_CUTOFF;
    let body = normalized(&comment.body);
    let shown = Shown::read(&body, old);
    let hiding = shown
        .hiding
        .iter()
        .find(|(_, cause)| !old || cause.unreadable());
    let number = &quoted(shown.number.as_deref()?);
    let fields: Vec<String> = shown.fields().iter().map(|l| quoted(l)).collect();
    let lines = fields.iter().map(String::as_str);
    let field = |name| lines.clone().find_map(|l: &str| l.strip_prefix(name));
    let (reviewers, range) = (field("Reviewers: "), field("Range: "));
    let findings = field("Findings: ");
    let breakerless = lines.clone().any(|l| l.starts_with("Breaker: skipped"));
    let binding = !old
        || [reviewers, range, findings].iter().any(Option::is_some)
        || hiding.is_some();
    let performance = |reviewers: Option<&BTreeSet<String>>| {
        let performer = reviewers
            .map_or_else(|| performer(&shown.text), |r| r.contains("performance"));
        (old && !performer && named(&shown.text)).then(|| {
            format!(
                "review round {number} names no performance, which this round requires."
            )
        })
    };
    let Ok(number) = number.parse::<u32>() else {
        let problem = format!("`## Review round {number}` has no round number");
        return Some(Parsed {
            round: Err(Malformed { binding, problem }),
            performance: performance(None),
        });
    };
    let missing = |name| {
        format!("review round {number} has no `{name}:` line. Write the round {FORMAT}")
    };
    let fields = || {
        if let Some(&(line, cause)) = hiding {
            return Err(cause.problem(number, line));
        }
        if let Some(lack) = lack(&shown.start).filter(|_| rated) {
            return Err(format!(
                "review round {number} {lack}. Write the round {FORMAT}"
            ));
        }
        let range = range.ok_or_else(|| missing("Range"))?.trim_matches('`');
        let (from, end) = range.split_once("..").ok_or_else(|| {
            format!(
                "review round {number} has the range `{range}`, not `<from>..<head>`"
            )
        })?;
        let findings = count(number, findings.ok_or_else(|| missing("Findings"))?)?;
        let tail = shown.end().map(|end| ends(end, number)).transpose()?;
        Ok(Round {
            number,
            reviewers: listed(reviewers.ok_or_else(|| missing("Reviewers"))?),
            breakerless,
            from: from.to_string(),
            end: end.to_string(),
            findings,
            fixed: fixed(&shown),
            ends: tail,
            posted: comment.created.clone(),
        })
    };
    let round = fields().map_err(|problem| Malformed { binding, problem });
    let performance = performance(round.as_ref().ok().map(|r| &r.reviewers));
    Some(Parsed { round, performance })
}

/// The count of the `Findings:` value `findings` of round `number`: `none` is 0.
fn count(number: u32, findings: &str) -> Result<u32, String> {
    if findings == "none" {
        return Ok(0);
    }
    findings.parse().map_err(|e| {
        format!(
            "review round {number} has `Findings: {findings}`, not a count or `none`: \
             {e}"
        )
    })
}

/// The lines of `text`, each ended by `\n`, `\r\n`, or a lone `\r`, as on GitHub.
fn lines(text: &str) -> impl Iterator<Item = &str> + Clone {
    text.split('\n')
        .flat_map(|l| l.strip_suffix('\r').unwrap_or(l).split('\r'))
}

/// What `start`, the start of a comment, lacks, as the words of a problem, or `None`.
/// Its first three lines are the name line (a bold session name, ` · `, and a role),
/// `Quality: <n>/10` with `n` from 1 to 10, and a summary line. A name, a role, or a
/// summary line that is [`blank`] is not one.
fn lack(start: &Start) -> Option<&'static str> {
    let mut lines = start.lines.iter().map(String::as_str);
    let role = start
        .session
        .as_deref()
        .zip(lines.next())
        .and_then(|(s, line)| {
            line.strip_prefix(s)
                .filter(|_| !blank(s))?
                .strip_prefix(" · ")
        });
    if role.is_none_or(blank) {
        return Some("does not start with a name line `**<session>** · <role>`");
    }
    let rating = lines
        .next()
        .and_then(|line| line.strip_prefix("Quality: ")?.strip_suffix("/10"));
    if !rating.is_some_and(|n| (1..=10).any(|rating| rating.to_string() == n)) {
        return Some(
            "has no line `Quality: <n>/10` with a rating from 1 to 10 after its \
             name line",
        );
    }
    if lines.next().is_none_or(blank) {
        return Some("has no summary line after its `Quality:` line");
    }
    None
}

/// The character that stands for an image in a line of [`texts`]. No text of a line
/// holds it, since it is [`ignorable`].
const IMAGE: char = '\u{FFF8}';

/// Whether `line`, a line of [`texts`], shows no text: it is empty, or holds only
/// images and the spaces between them.
fn blank(line: &str) -> bool {
    line.chars().all(|c| c == IMAGE || c == ' ')
}

/// The character that [`texts`] puts before a `GH-` that starts a text node or follows
/// an ignorable character. [`ends`] takes it out of the end lines and keeps its places
/// for [`issue`]. No text of a line holds it, since it is [`ignorable`].
const BREAK: char = '\u{FFF7}';

/// `line`, a line of [`texts`], as a [`Round`] and a problem hold it: each image is
/// `![]`, since [`IMAGE`] shows as nothing, and with no [`BREAK`].
fn quoted(line: &str) -> String {
    line.replace(IMAGE, "![]").replace(BREAK, "")
}

/// What the end lines `paragraph` of round `number` state. The paragraph must be the
/// [`END`] lines in order and nothing else. A [`LATER`] line after the first line is
/// not read.
fn ends(paragraph: &[String], number: u32) -> Result<Ends, String> {
    let [deferred, surface, hot] = END;
    let (values, read) = entries(paragraph, &[deferred, surface, hot, LATER]);
    let values = values
        .into_iter()
        .enumerate()
        .filter(|(i, (name, _))| *i == 0 || *name != LATER)
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    for (i, name) in END.into_iter().enumerate() {
        let found = values.get(i).map(|(found, _)| *found);
        if read < paragraph.len() || found != Some(name) {
            return Err(format!(
                "review round {number} does not end with a `{name}:` line. End each \
                 round with its `{deferred}:`, `{surface}:`, and `{hot}:` lines, in \
                 that order, {FORMAT}"
            ));
        }
    }
    let [(_, deferred), (_, surface), (_, hot)] =
        <[_; 3]>::try_from(values).map_err(|values| {
            let name = values[END.len()].0;
            format!(
                "review round {number} has a second `{name}:` line in its end lines."
            )
        })?;
    let (deferred, breaks) = unbroken(&deferred);
    Ok(Ends {
        deferred,
        breaks,
        surface: surface.replace(BREAK, ""),
        hot: stated(&hot),
    })
}

/// `value` with no [`BREAK`], and the place in it of each `BREAK` that `value` held.
fn unbroken(value: &str) -> (String, Vec<usize>) {
    let mut parts = value.split(BREAK);
    let mut text = parts.next().unwrap_or_default().to_owned();
    let mut breaks = Vec::new();
    for part in parts {
        breaks.push(text.len());
        text.push_str(part);
    }
    (text, breaks)
}

/// The first issue ([`issues`]) of `deferred`, the value of a `Deferred:` line with
/// the places `breaks` ([`Ends::breaks`]), with no OK: no comment link ([`links`])
/// starts after its start, and the words [`UNRISKY`] are not between it and the next
/// issue. A comment link that is an issue is no OK of itself.
fn unokayed(deferred: &str, breaks: &[usize]) -> Option<String> {
    let lower = deferred.to_ascii_lowercase();
    let ok = links(deferred).last();
    let mut issues = issues(deferred, breaks).peekable();
    while let Some(issue) = issues.next() {
        let next = issues.peek().map_or(deferred.len(), |next| next.start);
        let okayed = ok.is_some_and(|ok| ok > issue.start);
        if !okayed && !lower[issue.end..next].contains(UNRISKY) {
            return Some(deferred[issue].to_string());
        }
    }
    None
}

/// The place of each issue in `value`, the value of a `Deferred:` line with the places
/// `breaks`, in order: each issue ([`issue`]), and a comment link ([`link`]) with no
/// issue before it, which names the deferral by its comment. No issue starts in a
/// comment link or in another issue.
fn issues(value: &str, breaks: &[usize]) -> impl Iterator<Item = Range<usize>> {
    let mut end = 0;
    value.char_indices().filter_map(move |(at, _)| {
        if at < end {
            return None;
        }
        let comment = link(value, at);
        let first = end == 0;
        end = comment.or_else(|| issue(value, breaks, at))?;
        (comment.is_none() || first).then_some(at..end)
    })
}

/// The end of the issue that starts at `at` in `value`, a value with the places
/// `breaks` ([`Ends::breaks`]): `#`, or `GH-` in each ASCII case, and the digits after
/// it, or the URL of an issue or a PR ([`numbered`]). A `GH-` starts `value`, is at a
/// place of `breaks`, or follows a character that is no ASCII letter, no ASCII digit,
/// and no `_`, as GitHub reads it. The start of a comment link ([`link`]) is such a URL
/// too, so [`issues`] asks `link` first.
fn issue(value: &str, breaks: &[usize], at: usize) -> Option<usize> {
    let text = &value[at..];
    let before = value[..at].chars().next_back();
    let free = before.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
    let gh = text
        .split_at_checked(3)
        .filter(|(gh, _)| gh.eq_ignore_ascii_case("gh-"))
        .filter(|_| free || breaks.contains(&at));
    let number = text.strip_prefix('#').or(gh.map(|(_, number)| number));
    let url = text.strip_prefix(HOST);
    let rest = number
        .and_then(undigited)
        .or_else(|| url.and_then(numbered))?;
    Some(value.len() - rest.len())
}

/// `text` after the digits at its start, or `None` when it starts with no digit.
fn undigited(text: &str) -> Option<&str> {
    let rest = text.trim_start_matches(|c: char| c.is_ascii_digit());
    (rest.len() < text.len()).then_some(rest)
}

/// The problem when `surface`, the value of the `Public surface:` line of round
/// `number`, holds the word [`OWED`], in each ASCII case and with the characters at
/// its ends removed that are no letter and no number, or names items ([`stated`])
/// with no approval ([`approved`]).
fn unapproved(number: u32, surface: &str) -> Option<String> {
    let owed = |word: &str| {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        word.eq_ignore_ascii_case(OWED)
    };
    if surface.split(' ').any(owed) {
        return Some(format!(
            "review round {number} has the word `{OWED}` on its `Public surface:` \
             line. Get each approval, then put its link in the line."
        ));
    }
    (stated(surface) && !approved(surface)).then(|| {
        format!(
            "review round {number} names items on its `Public surface:` line with no \
             link to an approval, apart from a link after `{DEPARTS}`. {LINK}"
        )
    })
}

/// Whether `surface` links an approval: a comment link ([`links`]) with the words
/// [`DEPARTS`], in each ASCII case, at no place between it and the link before it.
fn approved(surface: &str) -> bool {
    let lower = surface.to_ascii_lowercase();
    let before = std::iter::once(0).chain(links(surface));
    before
        .zip(links(surface))
        .any(|(before, at)| !lower[before..at].contains(DEPARTS))
}

/// The start of each comment link ([`link`]) in `value`, in order.
fn links(value: &str) -> impl Iterator<Item = usize> {
    let hosts = value.match_indices(HOST).map(|(at, _)| at);
    hosts.filter(|&at| link(value, at).is_some())
}

/// The end of the comment link that starts at `at` in `value`: [`HOST`] and the path
/// of a comment ([`comment`]), at the start of `value` or after a space or `(`.
fn link(value: &str, at: usize) -> Option<usize> {
    let before = value[..at].chars().next_back();
    let free = before.is_none_or(|c| c == ' ' || c == '(');
    let path = value[at..].strip_prefix(HOST).filter(|_| free);
    Some(value.len() - path.and_then(comment)?.len())
}

/// The start of each comment link, and of each URL of an issue or a PR.
const HOST: &str = "https://github.com/";

/// `path` after the path of a comment on GitHub at its start: the path of an issue or
/// a PR ([`numbered`]), and `#issuecomment-`, `#pullrequestreview-`, or
/// `#discussion_r` with the digits after it.
fn comment(path: &str) -> Option<&str> {
    let marks = ["issuecomment-", "pullrequestreview-", "discussion_r"];
    let mark = numbered(path).and_then(|rest| rest.strip_prefix('#'));
    let id = mark.and_then(|mark| marks.iter().find_map(|m| mark.strip_prefix(m)));
    id.and_then(undigited)
}

/// `path` after the path of an issue or a PR on GitHub at its start: an owner, a
/// repository, `pull` or `issues`, and a number.
fn numbered(path: &str) -> Option<&str> {
    let mut parts = path.splitn(4, '/');
    let name = |name: &str| {
        let letter = |c: char| c.is_ascii_alphanumeric() || "-_.".contains(c);
        !name.is_empty() && name.chars().all(letter)
    };
    let named = parts.by_ref().take(2).all(name);
    let kind = parts.next().is_some_and(|k| k == "pull" || k == "issues");
    parts.next().filter(|_| named && kind).and_then(undigited)
}

/// Whether a round posted before [`CUTOFF`] names a hot path: a `Hot path:` line in
/// `text` ([`Shown::text`]) has a value ([`stated`]).
fn named(text: &[Vec<String>]) -> bool {
    states(text.iter().map(Vec::as_slice), END[2])
}

/// Whether a line of `paragraphs` starts with `name:` and has a value ([`stated`]).
fn states<'p>(paragraphs: impl IntoIterator<Item = &'p [String]>, name: &str) -> bool {
    paragraphs.into_iter().any(|p| {
        (0..p.len()).any(|i| {
            let (values, _) = entries(&p[i..], &[name]);
            values.first().is_some_and(|(_, value)| stated(value))
        })
    })
}

/// The entries of `names` at the start of `lines`, each as its name and value, and the
/// number of lines they take. An entry starts at the start of a line with its name,
/// and may wrap onto the lines after it.
fn entries<'n>(lines: &[String], names: &[&'n str]) -> (Vec<(&'n str, String)>, usize) {
    let mut values: Vec<(&str, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let name = names
            .iter()
            .find(|name| line.starts_with(&format!("{name}:")));
        match (name, values.last_mut()) {
            (Some(name), _) => values.push((*name, line[name.len() + 1..].to_string())),
            (None, Some((_, value))) => {
                *value = format!("{value} {line}");
            }
            _ => return (values, i),
        }
    }
    (values, lines.len())
}

/// Options for the GitHub extensions that change which lines are text or what they
/// show: tables, footnotes, task lists, strikethrough, and autolinks. Each footnote
/// stays in place ([`Shown::read`] moves it).
fn options() -> Options<'static> {
    let mut options = Options::default();
    let extension = &mut options.extension;
    extension.table = true;
    extension.footnotes = true;
    extension.tasklist = true;
    extension.strikethrough = true;
    extension.autolink = true;
    options.parse.leave_footnote_definitions = true;
    options
}

/// The start of a comment: the top-level paragraphs with which it starts.
#[derive(Debug, Default)]
struct Start {
    /// The text of the bold span that starts the comment, when its first block is a
    /// paragraph that starts with one.
    session: Option<String>,
    /// The lines of the paragraphs, as GitHub shows them ([`texts`]). A paragraph that
    /// comrak places in the wrong lines ([`misplaced`]) gives no line.
    lines: Vec<String>,
}

impl Start {
    /// Adds the top-level paragraph `paragraph`, with the lines `lines`.
    fn push<'n>(&mut self, paragraph: &'n AstNode<'n>, lines: &[String]) {
        if paragraph.previous_sibling().is_none() {
            self.session = session(paragraph);
        }
        self.lines.extend_from_slice(lines);
    }
}

/// A comment read as GitHub reads Markdown: its start before its round heading, its
/// text from the heading on, and the lines that can hide text or that the check cannot
/// read.
#[derive(Debug, Default)]
struct Shown<'a> {
    /// The round number of the first top-level heading that has one ([`number`]). When
    /// the comment is not old and has none, that of the comment respelled by
    /// [`unblocked`], since GitHub starts HTML blocks at other lines than comrak and
    /// can show a heading that comrak hides.
    number: Option<String>,
    /// The top-level paragraphs before the first block that is not a paragraph.
    start: Start,
    /// Each top-level block after the heading: the index in `text` of a paragraph, or
    /// `None` for any other block. The footnotes come last, as GitHub shows them.
    blocks: Vec<Option<usize>>,
    /// The lines of text of each paragraph after the heading, at any depth, as GitHub
    /// shows them ([`texts`]). A footnote with no reference is not shown. A paragraph
    /// that comrak places in the wrong lines ([`misplaced`]) gives no line.
    text: Vec<Vec<String>>,
    /// Each line of the comment that can hide text on GitHub or that the check cannot
    /// read, with its cause, in order.
    hiding: Vec<(&'a str, Cause)>,
    /// Whether the comment was posted before [`CUTOFF`], so the check reads no end
    /// lines in it.
    old: bool,
}

impl<'a> Shown<'a> {
    /// Reads `body` ([`normalized`]), posted before [`CUTOFF`] when `old`.
    fn read(body: &'a str, old: bool) -> Self {
        let mut shown = Self {
            old,
            ..Self::default()
        };
        let starts = starts(body);
        let line_start = |line: usize| starts[line - 1];
        let arena = Arena::new();
        let root = parse_document(&arena, body, &options());
        // The code blocks of a footnote that GitHub does not show still hold its lines.
        let codes = code_blocks(root, body, &starts);
        let nested = nested(root);
        append_notes(root);
        // Whether each top-level block so far is a paragraph. So it is false at a
        // paragraph inside another block.
        let mut unbroken = true;
        let mut hiding = Vec::new();
        for node in root.descendants() {
            let data = node.data.borrow();
            let start = line_start(data.sourcepos.start.line);
            let top = node.parent().is_some_and(|p| p.same_node(root));
            let index =
                matches!(data.value, NodeValue::Paragraph).then(|| shown.text.len());
            if top && shown.number.is_some() {
                shown.blocks.push(index);
            }
            unbroken &= !top || index.is_some();
            match &data.value {
                NodeValue::Heading(_) if top && shown.number.is_none() => {
                    shown.number = number(node);
                    shown.text.clear();
                }
                NodeValue::Paragraph if misplaced(node, data.sourcepos.end.line) => {
                    hiding.push((start, Cause::Misplaced));
                    shown.text.push(Vec::new());
                }
                NodeValue::Paragraph => {
                    let open = bracket(node, data.sourcepos.end.line);
                    hiding.extend(open.map(|line| (line_start(line), Cause::Bracket)));
                    let lines = texts(node);
                    if unbroken {
                        shown.start.push(node, &lines);
                    }
                    shown.text.push(lines);
                }
                NodeValue::HtmlBlock(_) | NodeValue::HtmlInline(_) => {
                    hiding.push((start, Cause::Raw));
                }
                _ => {}
            }
        }
        if shown.number.is_none() && !old {
            shown.number = respelled(&arena, body).children().find_map(number);
        }
        hiding.extend(tagged(body, &starts, &codes).map(|at| (at, Cause::Raw)));
        hiding.extend(nested.map(|line| (line_start(line), Cause::Nested)));
        hiding.sort_by_key(|(at, _)| *at);
        shown.hiding = hiding
            .into_iter()
            .map(|(at, cause)| (quoted_line(body, at), cause))
            .collect();
        shown
    }

    /// The lines of the fields: the first block after the round heading, or none when
    /// it is not a paragraph.
    fn fields(&self) -> &[String] {
        self.paragraph(self.blocks.first())
    }

    /// The end lines: the last block after the fields, or no lines when it is not a
    /// paragraph or the fields are the only block. `None` when the comment is old.
    fn end(&self) -> Option<&[String]> {
        (!self.old).then(|| self.paragraph(self.end_block().map(|i| &self.blocks[i])))
    }

    /// The blocks after the round heading other than that of the end lines.
    fn before_end(&self) -> &[Option<usize>] {
        &self.blocks[..self.end_block().unwrap_or(self.blocks.len())]
    }

    /// The index in [`Shown::blocks`] of the end lines: the last block after the
    /// fields, or none when the comment is old or the fields are the only block.
    fn end_block(&self) -> Option<usize> {
        (!self.old && self.blocks.len() > 1).then(|| self.blocks.len() - 1)
    }

    /// The lines of `block`, one of [`Shown::blocks`], or none when it is not a
    /// paragraph.
    fn paragraph(&self, block: Option<&Option<usize>>) -> &[String] {
        block
            .copied()
            .flatten()
            .map_or(&[][..], |i| self.text[i].as_slice())
    }
}

/// The text of the bold span that starts `paragraph`, or `None` when it starts with
/// none.
fn session<'n>(paragraph: &'n AstNode<'n>) -> Option<String> {
    let span = paragraph.first_child()?;
    let bold = matches!(span.data.borrow().value, NodeValue::Strong);
    bold.then(|| texts(span).join(" "))
}

/// The first line of `paragraph` before its last line `last` with a span of text that
/// holds `[^`. GitHub can read a `]` on a later line as the end of a footnote
/// reference, and then hides the text between them.
fn bracket<'n>(paragraph: &'n AstNode<'n>, last: usize) -> Option<usize> {
    paragraph.descendants().find_map(|span| {
        let data = span.data.borrow();
        let line = data.sourcepos.start.line;
        let text = matches!(&data.value, NodeValue::Text(t) if t.contains("[^"));
        (text && line < last).then_some(line)
    })
}

/// Whether comrak places each span of `paragraph` before its last line `last`. It
/// does after a link or an image with a line break after its text, and after a link
/// reference definition, and then each line it gives after them is wrong.
fn misplaced<'n>(paragraph: &'n AstNode<'n>, last: usize) -> bool {
    let mut spans = paragraph.descendants().skip(1);
    spans.all(|span| span.data.borrow().sourcepos.end.line < last)
}

/// Moves to the end of `root` each footnote definition that GitHub shows, where GitHub
/// shows it, and detaches each other one ([`footnotes`]).
fn append_notes<'n>(root: &'n AstNode<'n>) {
    for note in footnotes(root) {
        root.append(note);
    }
}

/// The line of the first footnote definition of `root` inside another one. GitHub
/// shows it as a footnote of its own, and comrak reads no reference to it. Call it
/// before [`footnotes`], which detaches the footnote that holds it.
fn nested<'n>(root: &'n AstNode<'n>) -> Option<usize> {
    let note = |n: &AstNode<'_>| {
        matches!(n.data.borrow().value, NodeValue::FootnoteDefinition(_))
    };
    root.descendants()
        .find(|n| note(n) && n.ancestors().skip(1).any(note))
        .map(|n| n.data.borrow().sourcepos.start.line)
}

/// Detaches from `root` each footnote definition that GitHub does not show, and gives
/// the others in body order. GitHub shows the first definition of each label to end,
/// when a reference names it.
fn footnotes<'n>(root: &'n AstNode<'n>) -> Vec<&'n AstNode<'n>> {
    let notes: Vec<_> = root
        .descendants()
        .filter_map(|node| match &node.data.borrow().value {
            NodeValue::FootnoteDefinition(note) => {
                Some((node, note.name.clone(), note.total_references))
            }
            _ => None,
        })
        .collect();
    // comrak counts the references of the last definition of a label, and GitHub
    // shows the first.
    let mut shown = Vec::new();
    for (i, (node, name, _)) in notes.iter().enumerate() {
        let label = |(_, other, _): &(_, String, _)| same(name, other);
        let first = !notes[..i].iter().any(label);
        let referenced = notes[i..]
            .iter()
            .any(|note @ (_, _, references)| *references > 0 && label(note));
        if first && referenced {
            shown.push(*node);
        } else {
            node.detach();
        }
    }
    shown
}

/// Whether comrak reads the footnote labels `a` and `b` as one label.
fn same(a: &str, b: &str) -> bool {
    let arena = Arena::new();
    let text = format!("[^{a}]\n\n[^{b}]: x");
    let root = parse_document(&arena, &text, &options());
    root.descendants()
        .any(|n| matches!(n.data.borrow().value, NodeValue::FootnoteReference(_)))
}

/// The lines of text of `paragraph` as GitHub shows them: its text and code spans
/// ([`shown`]), each link as its text and not its target, each footnote reference as
/// its number, each image as [`IMAGE`] in place of its text and line breaks, and each
/// run of white space as one space, trimmed. A character that looks like another stays
/// itself.
fn texts<'n>(paragraph: &'n AstNode<'n>) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut fresh = true;
    let image =
        |node: &'n AstNode<'n>| matches!(node.data.borrow().value, NodeValue::Image(_));
    for span in paragraph.descendants().skip(1) {
        if span.ancestors().skip(1).any(image) {
            continue;
        }
        let data = span.data.borrow();
        let text = match &data.value {
            NodeValue::SoftBreak | NodeValue::LineBreak => {
                fresh = true;
                continue;
            }
            NodeValue::Text(text) => shown(text),
            NodeValue::Code(code) => shown(&code.literal),
            NodeValue::Image(_) => IMAGE.to_string(),
            NodeValue::FootnoteReference(reference) => reference.ix.to_string(),
            _ => String::new(),
        };
        if fresh {
            lines.push(String::new());
            fresh = false;
        }
        if let Some(line) = lines.last_mut() {
            line.push_str(&text);
        }
    }
    lines
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// `text`, a text node or a code span, as GitHub shows it: with no invisible character
/// (Unicode default ignorable), and with [`BREAK`] before each `GH-`, in each ASCII
/// case, that starts it or follows an invisible character.
fn shown(text: &str) -> String {
    let mut shown = String::new();
    for part in text.split(ignorable) {
        if part
            .get(..3)
            .is_some_and(|gh| gh.eq_ignore_ascii_case("gh-"))
        {
            shown.push(BREAK);
        }
        shown.push_str(part);
    }
    shown
}

/// Whether `c` is a Unicode default ignorable code point, which shows as nothing.
fn ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{34F}'
            | '\u{61C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// `text` with each line ended by `\n`, and with each tab in the spaces, tabs, and
/// `>` at the start of a line, after its [`mark`], replaced by spaces to the next
/// multiple of 4 columns. Neither changes what GitHub shows. A line number then gives
/// the offset of its line, and comrak gives the right column after a tab that a quote
/// or a list item takes in part. The spaces and tabs at the end of a line stay: a line
/// of 1 to 3 spaces ends a footnote, and a backslash before a space is no line break.
fn normalized(text: &str) -> String {
    let mut normalized = String::with_capacity(text.len());
    let (mark, text) = text.split_at(mark(text));
    normalized.push_str(mark);
    for line in lines(text) {
        let content = line.trim_start_matches([' ', '\t', '>']);
        let start = normalized.len();
        for c in line[..line.len() - content.len()].chars() {
            match c {
                '\t' => {
                    let columns = 4 - (normalized.len() - start) % 4;
                    normalized.extend(std::iter::repeat_n(' ', columns));
                }
                c => normalized.push(c),
            }
        }
        normalized.push_str(content);
        normalized.push('\n');
    }
    normalized
}

/// The tree of `body` respelled by [`unblocked`]. Its source positions give the lines
/// of `body`, not the columns.
fn respelled<'a>(arena: &'a Arena<'a>, body: &str) -> &'a AstNode<'a> {
    parse_document(arena, &unblocked(body), &options())
}

/// `body` respelled so that comrak starts an HTML block only where GitHub does, which
/// reads HTML blocks by an older spec. At the start of each line ([`bare`]), each tag
/// whose name starts with a name that the two specs list in a different way is renamed
/// ([`RENAMED`]), and an invisible character ([`texts`]) is put before `<!` and a
/// lowercase letter, which starts no HTML block on GitHub.
fn unblocked(body: &str) -> String {
    let mut text = String::with_capacity(body.len());
    let (mark, body) = body.split_at(mark(body));
    text.push_str(mark);
    for line in body.split_inclusive('\n') {
        let rest = bare(line);
        text.push_str(&line[..line.len() - rest.len()]);
        let lowercase = rest
            .strip_prefix("<!")
            .is_some_and(|after| after.starts_with(|c: char| c.is_ascii_lowercase()));
        if lowercase {
            text.push('\u{200B}');
        }
        let name = rest.strip_prefix("</").or_else(|| rest.strip_prefix('<'));
        let renamed = name.and_then(|name| {
            let (from, to) = RENAMED.iter().find(|(from, _)| {
                name.get(..from.len())
                    .is_some_and(|tag| tag.eq_ignore_ascii_case(from))
            })?;
            Some((name, from, to))
        });
        match renamed {
            Some((name, from, to)) => {
                text.push_str(&rest[..rest.len() - name.len()]);
                text.push_str(to);
                text.push_str(&name[from.len()..]);
            }
            None => text.push_str(rest),
        }
    }
    text
}

/// Each tag name that starts an HTML block of type 6 in one of the two specs only, and
/// a name of the same length that does the same in comrak. GitHub's older spec lists
/// `source` and not `search`. `xearch` is in no list, so comrak starts a block there
/// only by type 7, as GitHub does at `search`. No list holds a longer name that starts
/// with one of these, so a longer name may be renamed too.
const RENAMED: [(&str, &str); 2] = [("source", "option"), ("search", "xearch")];

/// The round number of `heading`: the text after `Review round` when it is a heading
/// of level 2 whose first line of text as GitHub shows it ([`texts`]) is `Review
/// round` or starts with `Review round `.
fn number<'n>(heading: &'n AstNode<'n>) -> Option<String> {
    let value = &heading.data.borrow().value;
    let level = matches!(value, NodeValue::Heading(NodeHeading { level: 2, .. }));
    let text = level.then(|| texts(heading))?.into_iter().next()?;
    let rest = text.strip_prefix("Review round")?;
    let word = rest.is_empty() || rest.starts_with(' ');
    word.then(|| rest.trim_start().to_owned())
}

/// The length of the byte order mark at the start of `text`, or 0. comrak and GitHub
/// drop one such mark at the start of a comment, and no other.
fn mark(text: &str) -> usize {
    if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    }
}

/// The start of each line of `text`.
fn starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

/// The line of `body` that starts at `at`, with no [`mark`], trimmed.
fn quoted_line(body: &str, at: usize) -> &str {
    body[at.max(mark(body))..line_end(body, at)].trim()
}

/// The range in `body` of each code block of `root`, its tree, from its first character
/// to the end of its last line. `starts` holds the start of each line of `body`.
fn code_blocks<'n>(
    root: &'n AstNode<'n>,
    body: &str,
    starts: &[usize],
) -> Vec<Range<usize>> {
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            let Sourcepos { start, end } = data.sourcepos;
            let from = starts[start.line - 1] + start.column - 1;
            matches!(data.value, NodeValue::CodeBlock(_))
                .then(|| from..line_end(body, starts[end.line - 1]))
        })
        .collect()
}

/// The first of the line starts `starts` of `body` whose line, after its [`mark`], the
/// indent and the marks of quotes, list items, and footnote labels ([`note`]), starts
/// with raw HTML: `<` and a letter, `!`, `/`, or `?` that is not an autolink. A line in
/// a code block (`codes`) does not count. GitHub reads some of these lines as an HTML
/// block where comrak does not, such as `<source>`. GitHub reads the blocks of a
/// comment before its spans, so a line inside a code span, a link, or a link definition
/// counts too.
fn tagged(body: &str, starts: &[usize], codes: &[Range<usize>]) -> Option<usize> {
    starts.iter().copied().find(|&start| {
        let from = start.max(mark(body));
        let line = &body[from..line_end(body, start)];
        let rest = bare(line);
        let at = |rest: &str| from + line.len() - rest.len();
        !codes.iter().any(|code| code.contains(&at(rest))) && tag(rest)
    })
}

/// Whether `text` starts with `<` and a letter, `!`, `/`, or `?`, and not with an
/// autolink.
fn tag(text: &str) -> bool {
    let opens = |c: char| c.is_ascii_alphabetic() || "!/?".contains(c);
    let autolink = || {
        let arena = Arena::new();
        let root = parse_document(&arena, text, &options());
        root.descendants().any(|node| {
            let data = node.data.borrow();
            matches!(data.value, NodeValue::Link(_)) && data.sourcepos.start.column == 1
        })
    };
    text.strip_prefix('<').is_some_and(|l| l.starts_with(opens)) && !autolink()
}

/// `line` after the indent and the marks of quotes, list items, and footnote labels
/// ([`note`]) at its start.
fn bare(line: &str) -> &str {
    let mut rest = unmarked(line);
    while let Some(after) = note(rest) {
        rest = unmarked(after);
    }
    rest
}

/// `line` after the indent and the marks of quotes and list items at its start.
fn unmarked(line: &str) -> &str {
    let mut rest = line;
    loop {
        rest = rest.trim_start_matches([' ', '\t']);
        let number = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        let item = if number.len() < rest.len() {
            number.strip_prefix(['.', ')'])
        } else {
            rest.strip_prefix(['-', '+', '*'])
        };
        let item = item.filter(|after| after.starts_with([' ', '\t']));
        match item.or_else(|| rest.strip_prefix('>')) {
            Some(after) => rest = after,
            None => return rest,
        }
    }
}

/// The text after the footnote label at the start of `text`: `[^`, one or more
/// characters other than `]`, space, or tab, then `]:`.
fn note(text: &str) -> Option<&str> {
    let (label, after) = text.strip_prefix("[^")?.split_once(']')?;
    let label = !label.is_empty() && !label.contains([' ', '\t']);
    after.strip_prefix(':').filter(|_| label)
}

/// The offset in `text` of the end of the line that holds offset `start`.
fn line_end(text: &str, start: usize) -> usize {
    text[start..].find('\n').map_or(text.len(), |i| start + i)
}

/// Whether `value` states something: its first word, with its backticks and one final
/// `,`, `.`, or `;` removed, is not `none`.
fn stated(value: &str) -> bool {
    let first = value.split_whitespace().next().unwrap_or_default();
    let first = first.replace('`', "");
    first.strip_suffix([',', '.', ';']).unwrap_or(&first) != "none"
}

/// Reads the record of PR `pr` with `api`, and the state of each issue or PR that a
/// merge condition of its body names ([`conditions`]).
fn fetch(pr: &str, api: gh::Read<'_>) -> Result<Record, String> {
    let path = format!("repos/{{owner}}/{{repo}}/pulls/{pr}");
    let pull = api(&path)?;
    let files = api(&format!("{path}/files"))?;
    let comments = api(&format!("repos/{{owner}}/{{repo}}/issues/{pr}/comments"))?;
    let read = || -> Result<Record, String> {
        let pull = pull.first().ok_or("gh returned no PR")?;
        Ok(Record {
            branch: field::text(&pull["head"], "ref")?.to_string(),
            base: field::text(&pull["base"], "ref")?.to_string(),
            labels: field::list(pull, "labels")?
                .iter()
                .map(|l| field::text(l, "name").map(str::to_string))
                .collect::<Result<_, _>>()?,
            files: files
                .iter()
                .map(paths)
                .collect::<Result<Vec<_>, _>>()?
                .concat(),
            comments: comments
                .iter()
                .map(|c| {
                    Ok(Comment {
                        author: field::text(&c["user"], "login")?.to_string(),
                        body: field::text(c, "body")?.to_string(),
                        created: utc(field::text(c, "created_at")?)?.to_string(),
                    })
                })
                .collect::<Result<_, String>>()?,
            waits: waits(pull, api)?,
        })
    };
    read().map_err(|e| format!("PR {pr}: {e}"))
}

/// Each open issue that a merge condition of the body of `pull` names, read with
/// `api`, or the problem of a body that the check cannot read ([`conditions`]).
/// Errors when a read fails.
fn waits(pull: &Value, api: gh::Read<'_>) -> Result<Result<Vec<u64>, String>, String> {
    let body = if pull["body"].is_null() {
        ""
    } else {
        field::text(pull, "body")?
    };
    let found = match conditions(body) {
        Ok(found) => found,
        Err(problem) => return Ok(Err(problem)),
    };
    let mut open = Vec::new();
    for n in found {
        let issue = api(&format!("repos/{{owner}}/{{repo}}/issues/{n}"))?;
        let issue = issue.first().ok_or("gh returned no issue")?;
        if field::text(issue, "state")? == "open" {
            open.push(n);
        }
    }
    Ok(Ok(open))
}

/// Each issue or PR that a merge condition of `body` names, one time each, in body
/// order. A condition is `merges after` in each ASCII case, after no letter and no
/// digit, then white space and `#<n>`, in a paragraph as GitHub shows it ([`texts`]),
/// also of a list item or a block quote, and also in a code span. A code block, a
/// heading, a table, and HTML hold none. Only the issue right after the words counts.
/// Errors with the problem of a footnote inside a footnote ([`nested`]).
fn conditions(body: &str) -> Result<Vec<u64>, String> {
    let arena = Arena::new();
    let body = normalized(body);
    let root = respelled(&arena, &body);
    if let Some(line) = nested(root) {
        let (cause, remedy) =
            Cause::Nested.text(quoted_line(&body, starts(&body)[line - 1]));
        return Err(format!("the PR body has {cause}. {remedy}."));
    }
    footnotes(root);
    let paragraphs = root
        .descendants()
        .filter(|n| matches!(n.data.borrow().value, NodeValue::Paragraph));
    let mut found = Vec::new();
    for node in paragraphs {
        let text = texts(node).join(" ").to_ascii_lowercase();
        for (at, words) in text.match_indices("merges after #") {
            if text[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric)
            {
                continue;
            }
            let rest = &text[at + words.len()..];
            let digits = rest
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest.len());
            let n = rest[..digits].parse().ok();
            if let Some(n) = n.filter(|n| !found.contains(n)) {
                found.push(n);
            }
        }
    }
    Ok(found)
}

/// The paths of `file`, a file of a PR: its path before the PR when the PR moves it,
/// then its path.
fn paths(file: &Value) -> Result<Vec<String>, String> {
    let moved = file.get("previous_filename").is_some();
    let names = moved.then_some("previous_filename").into_iter();
    names
        .chain(["filename"])
        .map(|name| field::text(file, name).map(str::to_string))
        .collect()
}

/// `text` when it is a UTC time `YYYY-MM-DDTHH:MM:SSZ`, the form GitHub gives, in
/// which text order is time order.
fn utc(text: &str) -> Result<&str, String> {
    let shaped = text.len() == 20
        && text.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            10 => b == b'T',
            13 | 16 => b == b':',
            19 => b == b'Z',
            _ => b.is_ascii_digit(),
        });
    if shaped {
        Ok(text)
    } else {
        Err(format!(
            "`created_at` is `{text}`, not a UTC time `YYYY-MM-DDTHH:MM:SSZ`"
        ))
    }
}

#[cfg(test)]
mod tests;
