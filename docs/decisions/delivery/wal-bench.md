- **WAL BENCH (#324, 2026-10-08)** The cargo feature `sim` of `buffer`, off by default
  (`buffer`'s dev-dependency on itself turns it on for the bench), adds
  `#[doc(hidden)] pub mod bench` with `Ring { new, commit }` over `wal::Writer`, as
  STORED BENCH does for `home`. Only the benches of `buffer` (`test = true`) and its
  `alloc` and `memory` tests use it.
  `commit` appends its records, takes `trimmed`, syncs each record, and releases to the
  trimmed tail, so a time holds the writer's cost for each commit. The bench runs
  commits of 1 and 8 records, with the bench IDs 64 and 4096: 1 record gives the cost
  for each commit (`trimmed`, `release`), 8 the cost for each record (`append`,
  `synced`). Each ring has 4 blocks more than its bench ID (68 and 4100 blocks): with
  the three blocks that each record keeps free (#160), the trims come at most 19
  commits apart on the ring of 64 and 64 apart on the ring of 4096 (#2943). One
  `#[doc(hidden)] pub const PHASE` of `bench` holds the 4 blocks, and the bench and a
  test of `bench` read it. The test checks 10 laps of each ring after the warm-up.
  Both benches use the same rings.
  A commit trims only once for each step (STORE TRIM), so each sample of the `wal`
  benches holds `bench::SAMPLE_SIZE` (1000) commits, so at least 10 trims, and the
  `bench` tests check the most commits between trims. `deadline_trim` times only the
  deadline of 1 or 32 indexes that trims a step on the ring of ID 4096, after a warm-up
  past two laps that a test of `bench::Deadline` pins. Lost: the default sample, which
  holds 0 to 2 trims, so that its median moves with the phase of the trims.
  `SAMPLE_SIZE` is approved by `laptop.architect` (#2943, 2026-10-16T22:20:35Z):
  https://github.com/synnaxlabs/foundation/pull/2943#issuecomment-6225798040.
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
  `bench` also has `Deadline { new, commit }` over the private `State` of a buffer, from
  the child module `buffer::bench`, for the cases `deadline` of 1 and 32 indexes and
  `deadline_carry` of `benches/wal.rs`, on the ring of ID 64. `commit` appends one entry
  of 100 bytes of each index but the last, which appends one each `period` deadlines.
  Then it does the work of the append, the fit and take of the group, and then the work
  of the commit task at a deadline with `trimmed`, in the order of `run`:
  `State::deadline` (the take, the checkpoint, the carry record, and the seal of the
  records) and `State::synced`, with no file write. `new` refuses more than 54 indexes,
  whose group is over a body of two blocks. So a time holds the cost of the commit path
  of a buffer that trims for each deadline, and `deadline_carry` that of a carry record
  of 32 tails. `benches/logs.rs` also gets the case `lap_shrink` of 10000 paths, which
  times only the commit of `lap_end` that ends the lap of 2756 paths, each with a
  shrink, while the other paths wait. Lost: a sim `Buffer` over a sim file, timed from
  `append` to the end of the commit, whose file, executor, and clock step cost more than
  the code under test, while the `wal` benches already time the file write. Lost: a
  `pub(crate)` `State`, which widens `State` and each method that the bench calls, for
  one bench. Decided by `laptop.architect`: the surface of `Deadline` (#2909,
  2026-10-16T19:32:27Z):
  https://github.com/synnaxlabs/foundation/pull/2909#issuecomment-6223169251, with the
  refusal of more than 54 indexes and the seal of the records in `State::deadline`
  (2026-10-16T21:03:09Z):
  https://github.com/synnaxlabs/foundation/pull/2909#issuecomment-6224646122, and
  `lap_shrink` as item 6 of #2774, deferred with its OK (#2739, 2026-10-15T22:34:39Z):
  https://github.com/synnaxlabs/foundation/pull/2739#issuecomment-6203672199. The two
  lost designs have only the text approval (2026-10-16T20:11:06Z):
  https://github.com/synnaxlabs/foundation/pull/2909#issuecomment-6223804759.
