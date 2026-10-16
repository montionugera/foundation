- **POLL WRITER (2026-10-16)** `connector::poll::Writer` owns the frames of a kind
  that polls: one value of each channel per request, the stamp of the request in each
  index, a frame that leaves out each `None`, one write again of a group that the home
  refuses as `Backwards`, the samples lost when the pool has no block, and the counts
  of `poll::COUNTS` (`in` and `lost`). The kind keeps its loop and its errors, and
  calls the writer as a library. `connector-opcua` and `connector-modbus` write
  through it (`laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6207353409).
  `open` takes a channel of no fixed width, since the kind learns the sample type only
  after the open, and `write` panics on a value of it
  (https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6207638476).
  `open` refuses a channel that is an index with `Error::Config`
  (`connector.index`): the writer stamps each index, and only the open knows that a
  channel is an index. `open` panics on a channel named again: the parse of the kind
  refuses it, with the span of its block. `write` panics on a value that is not of the
  sample type of its channel: the kind maps each value by `Writer::types`, so a wrong
  type is a defect of the kind
  (https://github.com/synnaxlabs/foundation/pull/2817#issuecomment-6207963206).
