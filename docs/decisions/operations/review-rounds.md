- **REVIEW ROUNDS (2026-10-13)** A PR gets at most 3 review rounds, counted by the
  number in the round heading. A PR that replaces another continues its count. Each
  reviewer reports every finding that it can show in one round, and the author fixes
  every finding of a round in one batch before the next round. After round 3, no round
  runs. The author fixes each defect of round 3 (wrong behavior, data loss, a crash, or
  an oracle weakening) with a test that fails without the fix, and CI checks it. Each
  other finding is a deferral. The approvals, OKs, and links that "Done" of the `review`
  skill asks for stay. A PR whose last round was round 3 or later when this rule merged
  runs no new round. Reviewers put first what costs the most after merge, and a matter
  of style that leaves each statement true is no finding. The person, relayed by
  `laptop.monitor`
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6150302970,
  2026-10-13T16:27:23Z): "Cap reviews at 3 rounds. Make sure that agents are deeply
  aware that they only get 3 review rounds, so they need to bacth findings." and "We
  need to spent less time nitpicking bullshit, and more time focused on finding deep
  architectural flaws and problems." The person set the cap. The director decided the
  rest (https://github.com/synnaxlabs/foundation/pull/2586#issuecomment-6150355931,
  2026-10-13T16:35:25Z). Cost: a fix after round 3 has only its test and CI, and in 4 of
  the 5 worst escaped defects, the defect came in through a fix that nothing checked
  again. A style finding waits for the weekly `code-quality` pass. Supersedes, after
  round 3, "a code fix keeps its round" of TEXT FIXES. `cargo xtask review` reads this
  rule from #2585.
