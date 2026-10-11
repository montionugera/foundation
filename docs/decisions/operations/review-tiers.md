- **REVIEW TIERS (2026-10-06)** `reviewer` on every PR; `architecture` and `breaker` on
  every code PR; `performance` on hot paths, with measured numbers. A second round runs
  `reviewer` and `breaker` again on the fix commits only, with the earlier findings. A
  deferral in a risk crate needs the architect's explicit OK (the person, 2026-10-07:
  "YES"). 4 of the 5 worst escaped defects came in through a fix or a deferral that
  nothing checked again. Decided by the advisor under the quality delegation.
  Supersedes: BREAKER REVIEW.
  Amended (2026-10-10, TEXT FIXES): a commit that only applies `Text fixes:` items
  gets no second round.
  Amended (2026-10-11, REVIEW CHECK): a later round whose range changes a file under
  `docs/decisions/` also runs `architecture`, and the `review` check fails one that does
  not name it (the director,
  https://github.com/synnaxlabs/foundation/issues/1211#issuecomment-6106799591 and
  https://github.com/synnaxlabs/foundation/pull/2373#issuecomment-6106884396).
