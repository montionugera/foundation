- **ATTRIBUTION (2026-10-12)** `.claude/settings.json` gives `attribution.commit` and
  `attribution.pr` empty text, so Claude Code adds no Claude attribution line to a
  commit or a PR body. Rule 1 of `CLAUDE.md` has no check: the setting removes the
  cause, and a second check of the same line is defense in depth. A pushed commit with
  the line stays as it is (the person, 2026-10-02, relayed by `laptop.monitor` at
  2026-10-07T17:49:02Z,
  https://github.com/synnaxlabs/foundation/issues/1472#issuecomment-6043577200).
  Trigger: a Claude attribution line in a commit or a PR body that a session with this
  setting made reopens the check, from head `e04ad8b6e` of PR #2501. Ruled by
  `laptop.director` at 2026-10-12T22:55:40Z
  (https://github.com/synnaxlabs/foundation/pull/2501#issuecomment-6134192187).
  Supersedes the ruling of `laptop.director` at 2026-10-07T17:48:20Z
  (https://github.com/synnaxlabs/foundation/issues/1472#issuecomment-6043565737) and
  the sentence "#1472 already does the work at its cause" of its ruling at
  2026-10-12T22:42:56Z
  (https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6134029772).
