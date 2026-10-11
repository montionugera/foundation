- **STATUS CHANNELS (2026-10-06)** `node::status::TABLE` is the fixed set of a node's
  status channels: `clock.status` (`U8`: 0 unsynced, 1 synced, 2 holdover),
  `clock.offset` (`Span`), and `clock.error` (`Span`; an unknown error is the bound of
  `estimate::Measurement::unknown`). An unsynced clock gives no offset or error. Each
  table entry maps the pulled status to its value. A pure `Collector` pulls each value
  from a reader that its crate gives; no crate calls `node`. Lost: each crate pushes
  status events to a sink (BQ11b locks pull). Decided in #728.
  Amended (`laptop.architect-2`, 2026-10-11T06:01:45Z,
  https://github.com/synnaxlabs/foundation/issues/1229#issuecomment-6106059148): `node`
  shows a failed file call of the chunk store as status, which it pulls from the store.
  A node that runs does not stop for one, and `node::route` does not read the result of
  `blob::peer::serve` for it. A start fails for one: a store that does not open gives
  `Error::Blob`, and a put of a founding chunk that fails gives `Error::Mesh`, because
  the mesh does not open until its founding chunks are durable, and no telemetry runs
  yet. The store counts that put, and no channel shows it. Lost: `node::route` stops the
  node, or counts from the result of `blob::peer::serve` (one of the callers of the
  store). The table holds `blob.corruptions` and `blob.failures` (each `U64`), the
  counts of `blob::Status` after the open of the node's chunk store (BLOB STORE). The
  names and the type are in the body of #2362. The collector holds the store and reads
  it on the shard of the store, shard 0, because `blob` gives no reader for other
  threads (BLOB STORE). A node with no region has no member record (MEMBER RECORD), so
  it has no status channel and no collector. Lost: an optional store, with no sample for
  the two (no node collects with no store); the caller of `collect` pulls the status and
  passes it (the collector is the one place that pulls). Proposed in the plan of #2362
  (2026-10-11T08:21:23Z,
  https://github.com/synnaxlabs/foundation/issues/2362#issuecomment-6107079376); ruled
  by `laptop.architect-2` (2026-10-11T10:39:06Z,
  https://github.com/synnaxlabs/foundation/pull/2389#issuecomment-6108157810); the
  start, found by `laptop.architect`
  (https://github.com/synnaxlabs/foundation/pull/2389#issuecomment-6108181340), text by
  `laptop.architect-2` (2026-10-11T10:52:02Z,
  https://github.com/synnaxlabs/foundation/pull/2389#issuecomment-6108256946).
  The publish task (#728) is the one product caller of `Collector::collect`, with one
  call for each status period. The period is 100 ms or more, and the rate of the data
  does not change it, so `collect` runs at most 10 times each second and is not a hot
  path. Lost: a benchmark of `collect`. Ruled by `laptop.architect-2`
  (2026-10-11T11:43:20Z,
  https://github.com/synnaxlabs/foundation/pull/2389#issuecomment-6108659691).
