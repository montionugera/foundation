- **NARROW POLL (#2652, 2026-10-16)** A poll of many channels, one sample each, is the
  common write of a connector. For the cost of such a poll, a fix with no public change
  and no change to a disk or wire form goes ahead, and its PR states the cause and the
  figures before and after. A change to a form, or a target that a narrow sample does
  not meet, waits for the architect. Decided by `laptop.architect`
  (2026-10-14T02:05:47Z):
  https://github.com/synnaxlabs/foundation/issues/2652#issuecomment-6159271013.
  In #2929, the integer planner of BQ4 skips its statistics for a vector of one sample,
  so a narrow poll costs no more time than an f64 poll; no encoded byte changes. The
  `hub` bench `write` times one live write of 256 channels for each of six sample types.
  A `home` test states the stored body of that poll for each type: 34 B a data sample
  for types of 4 bytes or less and 42 B for 8-byte types. A second states the polls that
  fill a new 4 MiB ring: at one poll a commit, each poll takes 12 KiB, 48 B a data
  sample, for every type of 8 bytes or less. Both miss the P1 target of under 4 B a
  sample; #2928 holds that work.
