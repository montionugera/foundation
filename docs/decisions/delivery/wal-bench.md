- **WAL BENCH (#324, 2026-10-08)** The cargo feature `sim` of `buffer`, off by default
  (`buffer`'s dev-dependency on itself turns it on for the bench), adds
  `#[doc(hidden)] pub mod bench` with `Ring { new, commit }` over `wal::Writer`, as
  STORED BENCH does for `home`. Only the benches of `buffer` (`test = true`) and its
  `alloc` test use it.
  `commit` appends its records, takes `trimmed`, syncs each record, and releases to the
  trimmed tail, so a time holds the writer's cost for each commit. The bench runs
  commits of 1 and 8 records, with the bench IDs 64 and 4096: 1 record gives the cost
  for each commit (`trimmed`, `release`), 8 the cost for each record (`append`,
  `synced`). Each ring has 4 blocks more than its bench ID (68 and 4100 blocks): with
  the three blocks that each record keeps free (#160), each commit of 1 record trims
  at those sizes, as at 64 and 4096 before. One `#[doc(hidden)] pub const PHASE` of
  `bench` holds the 4 blocks, and the bench and a test of `bench` read it. The test
  checks 10 laps of each ring after the warm-up. Both benches use the same rings.
  The IDs stay, so the baseline compares the same work.
  Decided by `laptop.architect` (#2683, 2026-10-14T19:58:01Z):
  https://github.com/synnaxlabs/foundation/pull/2683#issuecomment-6177122793.
  `PHASE` is decided by `laptop.architect` (#2683, 2026-10-15T13:17:53Z):
  https://github.com/synnaxlabs/foundation/pull/2683#issuecomment-6193008995.
  `bench` also has `Group { new, commit }` over the groups of a buffer and
  `wal::Writer`, for the case `commit_group` of 1 and 32 entries of 100 bytes on the
  ring of ID 64. `commit` pushes the entries, which checks that the record fits,
  closes and seals the group, with the CRC of the body, then trims and syncs as
  `Ring` does, so a time holds the cost of the group commit path for each commit
  (#2699). Lost: a second job for `Ring`, which then holds a pool that its `commit`
  does not use. Decided by `laptop.architect` (#2688, 2026-10-15T20:11:59Z):
  https://github.com/synnaxlabs/foundation/pull/2688#issuecomment-6201316088.
  Lost: a copy of `wal.rs` in the bench through `#[path]`, which needs `entry`,
  `record`, and `crc32c` copied too and the workspace lints off; a time of
  `Buffer::append` and `committed` on a simulated file system, whose file writes hide
  the cost of the writer; and a control bench of code that a PR does not change, since
  the bench host gives A/A. It lands before the next PR after #1698 that changes
  `Writer::append`, `synced`, `release`, or `trimmed`. Decided by `laptop.architect`
  (2026-10-08T01:37:51Z):
  https://github.com/synnaxlabs/foundation/issues/324#issuecomment-6050388456. Its
  baseline is a quiet-host run of this bench at the head of #1729, not the #1698 rerun,
  whose scratch bench had no warm-up. The run makes two passes of one binary, and each
  median agrees within 3%. If one does not, a `crate:buffer` issue follows, and there is
  no baseline until it is fixed. Decided by `laptop.architect` (2026-10-08T03:12:46Z):
  https://github.com/synnaxlabs/foundation/pull/1729#issuecomment-6051403457.
  Supersedes "Its head numbers are the baseline of the `wal` benchmark" in
  https://github.com/synnaxlabs/foundation/pull/1698#issuecomment-6050306136. The
  #1698 numbers stay the record of the P1 judgment of #1698 only.
  `bench` also has `Logs { new, commit }` over `log::Logs`, for the bench
  `benches/logs.rs` (`test = true`). `commit` first hides the earlier records, as a
  trim that keeps up, then syncs one data entry of each index, so a time holds the
  cost of `Logs::sync` for each durable entry. Lost: a time of the commit on a
  simulated file system, whose file writes hide the sync. Lost: a time after a
  tagged entry in each log, as no sync of a data entry reads the tags. Decided by
  `laptop.architect` (#2236, 2026-10-10T05:43:41Z):
  https://github.com/synnaxlabs/foundation/pull/2236#issuecomment-6094318889. The
  time with no tag is decided by `laptop.architect` (#2236, 2026-10-10T05:58:44Z):
  https://github.com/synnaxlabs/foundation/pull/2236#issuecomment-6094432871.
  `bench` also has `Table { new, decode }`, for the bench `benches/decode.rs`
  (`test = true`). `new` writes a table of headers with the encoder of the crate, each
  of its own index, with the first header and each `empty`-th header after it with no
  samples. `decode` parses each header, as the scan of a read and the walk of an open
  do. Cases: 1023 headers with samples, and 1023 of which the first and every second
  after it, 512 in all, have no samples. Lost: a time of the whole read scan, whose
  waits and copies hide the decode. Asked by the round 2 `performance` report of #2816,
  which found slowdowns of the decode that differed by 4 to 6 times between two forms
  of a scratch bench (+0.09 against +0.40 ns per header with samples, and +0.11
  against +0.61 ns with half with no samples). Decided by `laptop.architect` (#2816,
  2026-10-16T06:47:29Z):
  https://github.com/synnaxlabs/foundation/pull/2816#issuecomment-6209801736.
  `bench::Logs` also has `fill(period, behind)`, as `trim(period, 0)` with the reach
  of the carry `behind` records back, for the case `fill` of `benches/logs.rs`: 1000
  indexes, one of them synced each 128 commits, a reach 64 records back, and the time
  of the one commit in 128 whose carry takes the tail of that index and fills from at
  most 4000 stale items after its reach: the first 4 of the 64 records and 4 items of
  the fifth. Lost: the mean of each commit, in which the hides of the commits that
  sync that index hide the fill.
  Decided by `laptop.architect` (#2796, 2026-10-16T07:33:04Z):
  https://github.com/synnaxlabs/foundation/issues/2796#issuecomment-6210474938, with
  the text approved (2026-10-16T07:45:51Z):
  https://github.com/synnaxlabs/foundation/issues/2796#issuecomment-6210673885.
