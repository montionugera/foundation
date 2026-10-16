- **POLL WRITER (2026-10-16)** `connector::poll::Writer` owns the frames of a kind
  that polls: one value of each channel per request, the stamp of the request in each
  index, a frame that leaves out each `None`, one write again of a group that the home
  refuses as `Backwards`, the samples lost when the pool has no block, and the counts
  of `poll::COUNTS` (`in` and `lost`). The kind keeps its loop and its errors, and
  calls the writer as a library. `connector-opcua` writes through it, and
  `connector-modbus` will in PR B2 of #432 (`laptop.architect-2`,
  2026-10-16T03:23:05Z:
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6207353409).
  `open` takes a channel of no fixed width, since the kind learns the sample type only
  after the open, and `write` panics on a value of it
  (`laptop.architect-2`, 2026-10-16T03:48:17Z, item 3:
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6207638476).
  `open` refuses a channel that is an index with `Error::Config`
  (`connector.index`): the writer stamps each index, and only the open knows that a
  channel is an index. That error comes before each check of the kind, so it comes
  alone. `open` panics on a channel named again: the parse of the kind
  refuses it, with the span of its block. `write` panics on a value that is not of the
  sample type of its channel: the kind maps each value by `Writer::types`, so a wrong
  type is a defect of the kind
  (`laptop.architect-2`, 2026-10-16T04:17:23Z, items 1 and 2:
  https://github.com/synnaxlabs/foundation/pull/2817#issuecomment-6207963206).
  `status::Writer` keeps its own write again: it writes string series, stages its
  values again when the pool has no block, and closes on `Removed`, where
  `poll::Writer` writes fixed-width samples, loses the frame, and gives the error to
  the run. One writer for both is deferred to #2527. Lost: a copy of the writer in
  each kind, which the rule of `Backwards` and of pool loss would drift between; a
  `write` that takes a closure per channel, which costs a call per sample
  (`laptop.architect-2`, 2026-10-16T04:07:48Z:
  https://github.com/synnaxlabs/foundation/pull/2817#issuecomment-6207854657); a bit
  set of the kept channels beside the values, which each caller keeps in step; and an
  error at the open for a channel named again, which the parse refuses first.
