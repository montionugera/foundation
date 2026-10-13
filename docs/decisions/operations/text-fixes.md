- **TEXT FIXES (2026-10-10)** A commit that only applies `Text fixes:` items starts no
  review round. A finding in text only (a `.md` file, a `.rs` comment line that,
  trimmed, starts with `//` and is not a doctest line, the PR title or body, an end line
  of a round comment, or the text with which the author states a later step of the PR or
  a deferral that is not in a risk crate, other than a test in it), at any severity,
  whose fix the `reviewer` gives as the exact new text, is a `Text fixes:` item, not a
  finding. That text of a deferral or a later step is the body of the issue that the
  author files for it, or the author's comment on an existing issue, never text that
  another session wrote. A test in it is the code of the test, and each sentence that
  states what the test asserts or what must pass. A finding in that body or comment is
  no `Text fixes:` item when the body or comment states a deferral in a risk crate, also
  when it states a later step or another deferral. The author applies an item as given,
  and the `reviewer` of the next round, when one runs, checks it where the author
  applied it. An item that gives only facts and links, or that asks the author to find a
  fact or give a reason, is a finding, and its fix gets a round. When that fix changes
  no code, the round runs `reviewer` alone. A text item still needs each approval that
  its file needs. In the range after a review round (REVIEW CHECK), code is each change
  to a file that is not a `.md` file, other than a `.rs` line that, trimmed, is empty or
  starts with `//`. The last round may end at a commit from which only clean merges of
  the base and commits that apply its `Text fixes:` items reach the head. `cargo xtask
  review` checks that each merge after the last round is a clean merge of the base (`git
  merge-tree`), that the commits after it change no code, and, when one that is not a
  merge follows it, that its round comment has a line `Text fixes:` that is not `Text
  fixes: none` (#2315). After a last round of 3 or more (REVIEW ROUNDS), it checks only
  the merges and each change to a decision. It cannot check that a commit applies only
  those items. Evidence (`laptop.monitor`, 102 PRs merged from 2026-10-09T00:00Z): a
  median of 7 rounds per PR, about 44% of the findings in text, and each text fix moved
  the head and started a round. Late rounds still found defects in code (#2191, #2003),
  so a code fix keeps its round. The person, at 2026-10-10T15:50:57Z, in the
  `laptop.monitor` chat
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
  doctest lines out of the items. The director, at 2026-10-11T05:30:51Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6105850818), added
  the body of an issue or a comment that holds a deferral or a later step, other than a
  test in it: round 12 of #2331 ran for one paragraph of #2355 whose new text the
  `reviewer` of round 11 gave, and round 10 found that the test of #2355 did not
  compile. The director, at 2026-10-11T05:56:19Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6106022120),
  replaced that place with the text with which the author states the deferral or the
  step, because the first text let an item change the body of an issue that another
  session wrote (#2324). The same decision states what a test in that text is, asks for
  the OK of the architect again in a risk crate, and makes the `reviewer` check each
  item where the author applied it. The director, at 2026-10-11T06:21:13Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6106196091), took
  the text of a deferral in a risk crate out of the text places and removed that second
  OK, which had no place in a round comment: a finding in that text gets a round, as on
  `main`. The director, at 2026-10-11T06:41:21Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6106336431), made a
  text that states such a deferral no text place also when it states a later step or
  another deferral, because one comment can state both
  (https://github.com/synnaxlabs/foundation/issues/2105#issuecomment-6105778995). The
  director, at 2026-10-11T07:02:40Z
  (https://github.com/synnaxlabs/foundation/issues/2041#issuecomment-6106487131),
  limited that sentence to the issue body or the comment of the author, because its
  words "A text" also covered a `.md` file and an end line of a round comment that state
  such a deferral, and put it after the sentence on a test, because "A test in it" then
  read as a test in that text. Ruling point 3 and the change to #2315 at
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
