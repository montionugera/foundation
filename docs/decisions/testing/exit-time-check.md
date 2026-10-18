- **EXIT TIME CHECK (2026-10-18)** Exit test 1 of ONE NODE checks the time of each
  sample that InfluxDB holds against the time at which the server answered its read.
  The offset is the stored time less the served time. The window `W_k` of read `k` is
  the served time of read `k+1` less that of read `k-1`; for read 0, from the start of
  the node. Each sample holds `|offset| <= W_k / 2 + BOUND` (the hold): the stamp is
  the midpoint of its read (STAMP), and the read lies inside its window. Each sample of
  a read after read 0 whose window is at most `2 * BOUND` holds `|offset| <= BOUND`, the
  1 s target of the MVP (the target). A wider window shows a read too slow for the
  target, and no stamp rule meets it with one request. Read 0 takes only the hold,
  since its window holds the start of the node. A failure gives the offset and window of
  each checked sample. After #2627, the hold takes the error that the node writes for
  the sample in place of `W_k / 2`. A clock stall at the start of the node gets its own
  issue when a failure shows it, with its plan to the architect of `clock`.
  Decided by `laptop.architect-2` (2026-10-18T16:15:17Z):
  https://github.com/synnaxlabs/foundation/issues/3198#issuecomment-6252160050. Lost: a
  bracket, the stamp of read `k` between the served times of reads `k-1` and `k+1` plus
  or minus `BOUND`, which lets the offset grow with each stall and drops the 1 s target.
