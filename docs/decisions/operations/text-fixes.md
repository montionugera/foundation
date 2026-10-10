- **TEXT FIXES (2026-10-10)** A fix that changes no code starts no review round. Code is
  each change to a file that is not a `.md` file, other than a `.rs` line that, trimmed,
  is empty or starts with `//`. A finding in text only (a `.md` file, a `.rs` comment,
  the PR title or body, or an end line of a round comment), at any severity, whose fix
  the `reviewer` gives as the exact new text, or as each fact and link that the text
  must state, is a `Text fixes:` item, not a finding. The author applies it as given,
  and the `reviewer` of the next round, when one runs, checks it. An item that asks the
  author to find a fact or give a reason stays a finding, because no reviewer gave its
  text. A text item still needs each approval that its file needs. The last round may
  end at a commit from which only clean merges of the base and commits that change no
  code reach the head, and `cargo xtask review` checks it (#2315). Evidence
  (`laptop.monitor`, 102 PRs merged from 2026-10-09T00:00Z): a median of 7 rounds per
  PR, about 44% of the findings in text, and each text fix moved the head and started a
  round. Late rounds still found defects in code (#2191, #2003), so a code fix keeps
  its round. The person, at 2026-10-10T15:50:57Z, in the `laptop.monitor` chat
  (https://github.com/synnaxlabs/foundation/issues/1703#issuecomment-6103074621): "Yes,
  let's do that please". Supersedes: the drop of change 1 in COST TRIALS. Amends REVIEW
  TIERS: a fix commit that changes no code gets no second round.
