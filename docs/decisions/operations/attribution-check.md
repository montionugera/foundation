- **ATTRIBUTION CHECK (2026-10-12)** Rule 1 of `CLAUDE.md` has a check. A
  `PreToolUse` hook on `Bash` in `.claude/settings.json` runs `cargo xtask attribution`
  when a command has `git` and then `push`, and refuses the call when it fails. The
  command fails for each commit of `HEAD` on no remote branch, merges too, whose
  message has a line that starts with `Co-Authored-By:` and names Claude or
  `noreply@anthropic.com`, or a line with "Generated with Claude Code" or "Generated
  with [Claude Code]", in any case. `cargo xtask review` fails for each such line in
  the PR body, and reads no commit. A pushed commit is never rewritten, not even to fix
  Rule 1 (the person, 2026-10-02, relayed by `laptop.monitor`,
  https://github.com/synnaxlabs/foundation/issues/1472#issuecomment-6043577200), so a
  check of pushed commits would fail a PR that no session may fix. Ruled by
  `laptop.director`
  (https://github.com/synnaxlabs/foundation/issues/1472#issuecomment-6043565737 and
  https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6134029772).
  `.claude/settings.json` also sets the `attribution` of commits and PRs to empty
  text, so Claude Code does not tell a session to add the line.
