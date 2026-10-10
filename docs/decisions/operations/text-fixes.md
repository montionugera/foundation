- **TEXT FIXES (2026-10-10)** A commit that only applies `Text fixes:` items starts no
  review round. A finding in text only (a `.md` file, a `.rs` comment line that,
  trimmed, starts with `//`, the PR title or body, or an end line of a round comment),
  at any severity, whose fix the `reviewer` gives as the exact new text, is a
  `Text fixes:` item, not a finding. The author applies it as given, and the
  `reviewer` of the next round, when one runs, checks it. An item that gives only facts
  and links, or that asks the author to find a fact or give a reason, is a finding, and
  its fix gets a round. When that fix changes no code, the round runs `reviewer` alone.
  A text item still needs each approval that its file needs. In the range after a
  review round (REVIEW CHECK), code is each change to a file that is not a `.md` file,
  other than a `.rs` line that, trimmed, is empty or starts with `//`. The last round
  may end at a commit from which only clean merges of the base and commits that apply
  its `Text fixes:` items reach the head. `cargo xtask review` checks that those
  commits change no code and that the round comment has a `Text fixes:` line (#2315).
  It cannot check that a commit applies only those items. Evidence (`laptop.monitor`,
  102 PRs merged from 2026-10-09T00:00Z): a median of 7 rounds per PR, about 44% of the
  findings in text, and each text fix moved the head and started a round. Late rounds
  still found defects in code (#2191, #2003), so a code fix keeps its round. The
  person, at 2026-10-10T15:50:57Z, in the `laptop.monitor` chat
  (https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621): "Yes,
  let's do that please". The director decided the items, the code test, and one
  departure from that recommendation: text that the author writes gets a round with
  `reviewer` alone, because in #1471 an applied text dropped a reason
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6103180556).
  Supersedes: the drop of change 1 in COST TRIALS (the person, 2026-10-08T01:17Z,
  recorded in https://github.com/synnaxlabs/foundation/issues/1703). Amends REVIEW
  TIERS: a commit that only applies `Text fixes:` items gets no second round.
