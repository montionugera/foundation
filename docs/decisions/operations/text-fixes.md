- **TEXT FIXES (2026-10-10)** A commit that only applies `Text fixes:` items starts no
  review round. A finding in text only (a `.md` file, a `.rs` comment line that,
  trimmed, starts with `//` and is not a doctest line, the PR title or body, or an end
  line of a round comment), at any severity, whose fix the `reviewer` gives as the exact
  new text, is a `Text fixes:` item, not a finding. The author applies it as given, and
  the `reviewer` of the next round, when one runs, checks it. An item that gives only
  facts and links, or that asks the author to find a fact or give a reason, is a
  finding, and its fix gets a round. When that fix changes no code, the round runs
  `reviewer` alone. A text item still needs each approval that its file needs. In the
  range after a review round (REVIEW CHECK), code is each change to a file that is not a
  `.md` file, other than a `.rs` line that, trimmed, is empty or starts with `//`. The
  last round may end at a commit from which only clean merges of the base and commits
  that apply its `Text fixes:` items reach the head. `cargo xtask review` checks that
  each merge after the last round is a clean merge of the base (`git merge-tree`), that
  the commits after it change no code, and, when one that is not a merge follows it,
  that its round comment has a line `Text fixes:` that is not `Text fixes: none`
  (#2315). It cannot check that a commit applies only those items. Evidence
  (`laptop.monitor`, 102 PRs merged from 2026-10-09T00:00Z): a median of 7 rounds per
  PR, about 44% of the findings in text, and each text fix moved the head and started a
  round. Late rounds still found defects in code (#2191, #2003), so a code fix keeps its
  round. The person, at 2026-10-10T15:50:57Z, in the `laptop.monitor` chat
  (https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621): "Yes,
  let's do that please". The director decided the items and the code test at
  2026-10-10T23:02:11Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6103117993), and at
  2026-10-10T23:10:22Z replaced them where they differ
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6103180556): an
  item gives the exact new text, and the fix of a finding that changes no code gets a
  round with `reviewer` alone, because in #1471 an applied text dropped a reason. The
  director, in round 2 of #2316 at 2026-10-10T23:20:44Z
  (https://github.com/synnaxlabs/foundation/pull/2316#issuecomment-6103259545), left
  doctest lines out of the items. Ruling point 3 and the change to #2315 at
  2026-10-10T23:10:40Z
  (https://github.com/synnaxlabs/foundation/issues/2315#issuecomment-6103182934) added
  the `Text fixes:` line check. The change at 23:21:22Z
  (https://github.com/synnaxlabs/foundation/issues/2315#issuecomment-6103264281)
  corrected point 3: the line is needed only when a commit that is not a merge follows
  the last round, and `Text fixes: none` does not count. The change at 23:29:42Z
  (https://github.com/synnaxlabs/foundation/issues/2315#issuecomment-6103326270) kept
  the check that each merge is a clean merge of the base. Supersedes: the drop of change
  1 in COST TRIALS (the person, 2026-10-08T01:17Z, recorded in
  https://github.com/synnaxlabs/foundation/issues/1703). Amends REVIEW TIERS: a commit
  that only applies `Text fixes:` items gets no second round.
