- **REVIEW ROUNDS (2026-10-13)** A PR gets at most 3 review rounds, counted by the
  number in the round heading. Each reviewer reports every finding that it can show in
  one round, and the author fixes every finding of a round in one batch before the next
  round. A round comment that only holds a late report, or only checks a merge of `main`
  that is not clean or a merge and the commits that fix what it broke, takes the number
  of the round comment before it, adds its findings to that round, and does not count. A
  PR that replaces another before round 3 continues its count, and its first round
  reviews its whole diff with each reviewer of round 1. After round 3, no other round
  runs, and no PR replaces it, other than a new design that the crate's architect
  approves, which starts at round 1. The author fixes each defect of round 3 (wrong
  behavior, data loss, a crash, or an oracle weakening) with a test that fails without
  the fix, and CI checks it. Each other finding is a deferral. The deferral of a finding
  that changes a public surface, a crate boundary, or a decision needs the explicit OK
  of the crate's architect, or the PR closes for the new design. After round 3, the PR
  changes only by merges of `main` and the commits that fix what one broke, the fixes of
  defects, and the `Text fixes:` items of round 3, and an `architecture` run names the
  public surface and hot path of the fixes. The approvals, OKs, links, and `performance`
  runs that the `review` skill asks for stay, also for a fix after round 3. A PR whose
  last round was round 3 or later when this rule merged runs no new round, and its
  changes after that round meet the same limit. Reviewers put first what costs the most
  after merge, and a matter of style that leaves each statement true is no finding. The
  person, relayed by `laptop.monitor`
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6150302970,
  2026-10-13T16:27:23Z): "Cap reviews at 3 rounds. Make sure that agents are deeply
  aware that they only get 3 review rounds, so they need to bacth findings." and "We
  need to spent less time nitpicking bullshit, and more time focused on finding deep
  architectural flaws and problems." The person set the cap. The director decided the
  rest (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150355931,
  2026-10-13T16:35:25Z), and the late reports, merges, replacements, and limits after
  round 3 in round 1 of #2586
  (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150615724,
  2026-10-13T16:49:41Z), and the design flaws, merge breaks, findings of a round, and
  the range after round 3 in round 2
  (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150915157,
  2026-10-13T17:06:12Z). Cost: a fix after round 3 gets no `reviewer` or `breaker`, only
  its test, CI, an `architecture` run, and the approvals and `performance` runs that
  stay, and in 4 of the 5 worst escaped defects, the defect came in through a fix or a
  deferral that nothing checked again. A style finding waits for the weekly
  `code-quality` pass. Supersedes, after round 3, "a code fix keeps its round" of TEXT
  FIXES (the person, 2026-10-10T15:50:57Z,
  https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621) and "A
  second round runs `reviewer` and `breaker` again on the fix commits only" of REVIEW
  TIERS. `cargo xtask review` checks this rule from #2585 (REVIEW CHECK), other than
  what the PR changes after round 3.
