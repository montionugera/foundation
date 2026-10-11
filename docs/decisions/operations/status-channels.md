- **STATUS CHANNELS (2026-10-06)** `node::status::TABLE` is the fixed set of a node's
  status channels: `clock.status` (`U8`: 0 unsynced, 1 synced, 2 holdover),
  `clock.offset` (`Span`), and `clock.error` (`Span`; an unknown error is the bound of
  `estimate::Measurement::unknown`). An unsynced clock gives no offset or error. Each
  table entry maps the pulled status to its value. A pure `Collector` pulls each value
  from a reader that its crate gives; no crate calls `node`. Lost: each crate pushes
  status events to a sink (BQ11b locks pull). Decided in #728.
  Amended (`laptop.architect-2`, 2026-10-11T06:01:45Z,
  https://github.com/synnaxlabs/foundation/issues/1229#issuecomment-6106059148): the
  table also holds `blob.corruptions` and `blob.failures` (each `U64`), the counts of
  `blob::Status` after the open of the node's chunk store (BLOB STORE). The node does
  not stop for a failure of the chunk store. Lost: `node::route` stops the node, or
  counts from the result of `blob::peer::serve` (one of the callers of the store). The
  collector holds the store and reads it on its shard, shard 0. A node with no region
  has no chunk store, and gives no sample for the two. Lost: a count of 0 for a node
  with no store (it says that a store is there). Decided by the builder (#2362,
  https://github.com/synnaxlabs/foundation/issues/2362#issuecomment-6107079376).
