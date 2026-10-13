- **REVIEW ROUNDS (2026-10-13)** A PR gets at most 3 review rounds, counted by the
  number in the round heading. Each reviewer reports every finding that it can show in
  one round, and the author fixes every finding of a round in one batch before the next
  round. A round comment that only holds a late report, or only checks a merge of `main`
  that is not clean or a merge and the commits that fix what it broke, takes the number
  of the round comment before it, adds its findings to that round, and does not count.
  The end of a round is the end of the range of its first round comment, and the next
  round and the range after round 3 start there. A PR that replaces another before round
  3 continues its count, and its first round reviews its whole diff with each reviewer
  of round 1. A design flaw is a finding whose fix changes a public surface, a crate
  boundary, a decision, or the owner of state. The design approver of an item is the
  crate's architect, `laptop.architect` for a contract between the two lists, or
  `laptop.director` for any other item. When the crate's architect wrote the PR, it is
  the other architect. When `laptop.director` wrote the PR, it is `laptop.director`,
  which sends each OK and approval that it gives to `laptop.monitor` at once. They go
  ahead unless the person objects, and the person approves the PR before it merges.
  After round 3, no other round runs, and no PR replaces it, other than a new design
  that its design approver approves, which starts at round 1. Each round comment
  numbered 3 marks each finding `defect` (wrong behavior, data loss, a crash, or an
  oracle weakening), `design flaw`, or neither, as its reviewer marked it. The author
  fixes each defect with a test that fails without the fix, CI checks it, and a defect
  is never deferred. Each other finding is a deferral, and the deferral of a design flaw
  needs the explicit OK of its design approver, or the PR closes for the new design.
  After round 3, the PR changes only by these changes: merges of `main` and the commits
  that fix what one broke, the fixes of defects, the `Text fixes:` items of round 3, and
  each change that an approval or OK of the `review` skill asks for, with the link to
  the comment that asks for it. When the PR changes by more than clean merges and text
  fixes, an `architecture` run on the whole range after round 3 names its public surface
  and hot path. A defect that the run finds gets its fix, its test, and a new run. Each
  item of that surface, and each decision or public doc that states what a crate does
  and that the range changes, needs the approval of its design approver and each
  approval that the `review` skill asks for. The approvals, OKs, links, and
  `performance` runs that the `review` skill asks for stay. A PR whose last round was
  round 3 or later when this rule merged runs no new round. A `reviewer` marks the
  findings of that round, and its changes after that round meet the same limit.
  Reviewers put first what costs the most after merge, and a matter of style that leaves
  each statement true is no finding. The person, relayed by `laptop.monitor`
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6150302970,
  2026-10-13T16:27:23Z): "Cap reviews at 3 rounds. Make sure that agents are deeply
  aware that they only get 3 review rounds, so they need to bacth findings." and "We
  need to spent less time nitpicking bullshit, and more time focused on finding deep
  architectural flaws and problems." The person set the cap. The director decided these
  parts:
  - the rest
    (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150355931,
    2026-10-13T16:35:25Z);
  - the late reports, merges, replacements, and limits after round 3, in round 1 of
    #2586 (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150615724,
    2026-10-13T16:49:41Z);
  - the design flaws, merge breaks, findings of a round, and the range after round 3, in
    round 2 of #2586
    (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150915157,
    2026-10-13T17:06:12Z);
  - the end of a round, the design approver, the marks, the changes that an approval
    asks for, and the run after round 3, in round 3 of #2586
    (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6151416969,
    2026-10-13T17:34:11Z);
  - the design approver of a PR that `laptop.director` wrote, to fit PERSON DECIDES THE
    DESIGN after the person's words of 2026-10-13T19:52:40Z
    (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6153919476).

  Cost: a fix after round 3 gets no `reviewer` or `breaker`, only its test, CI, an
  `architecture` run, and the approvals and `performance` runs that stay. In 4 of the 5
  worst escaped defects, the defect came in through a fix or a deferral that nothing
  checked again. A style finding waits for the weekly `code-quality` pass. Supersedes,
  after round 3, "a code fix keeps its round" of TEXT FIXES (the person,
  2026-10-10T15:50:57Z,
  https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621) and "A
  second round runs `reviewer` and `breaker` again on the fix commits only" of REVIEW
  TIERS. From #2585 (REVIEW CHECK), `cargo xtask review` checks that no round number
  goes past 3, the reviewers of each round comment, the findings of the last round below
  round 3 across its round comments, the end of the last round comment, the OK links in
  a risk crate, the approvals of the `Public surface:` line, and, after round 3, an item
  on that line for each change to a decision. It does not check the marks, the other
  changes in the range after round 3, what a round comment with the number of the one
  before it holds, that a PR after round 3 is a new design, or the OK of a deferred
  design flaw outside a risk crate.
