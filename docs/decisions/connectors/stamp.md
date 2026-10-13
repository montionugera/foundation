- **STAMP (#435)** `connector::stamp::Stamps` stamps the frames of one index of a
  kind that reads by request. The stamp of a frame is the midpoint of the mesh times
  before and after the request that read its samples, or 1 ns after the stamp before
  it when that is later, so the home never refuses it as backwards in one run. After
  a refusal as backwards, the frame is written again 1 ns after the stamp that the
  refusal gives. `now` is a closure, so a test needs no hub. The status writer of
  the supervisor stamps its frames with `Stamps::now`, the time of the hub or the
  nanosecond after the stamp before it, so one module holds the stamp rule
  (`laptop.architect-2`, 2026-10-13:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6135871024). The
  person put the module in `connector` for each kind that reads by request (ONE NODE
  record, section 3,
  https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6135595217). Lost:
  the stamp at the answer (it is late by the whole answer time); the module in
  `connector-opcua` (each later kind that reads by request copies it); a method on
  `hub::writer::Writer` (the hub then knows that a request has two ends).
