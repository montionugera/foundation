- **INFLUX KIND** `connector_influx::Kind` reads `address` and the reader settings
  (READER SETTINGS). `address` is an `http::Uri`, since a `Name` is a mesh name. `parse`
  reads `address` through `connector::http::uri`, so a plan finds an address that
  `send` refuses before a run. `discover` gives no documents. Decided by
  `laptop.architect-2` on #1153
  (https://github.com/synnaxlabs/foundation/issues/1153#issuecomment-6051297152,
  2026-10-08 03:02 UTC). The kind also refuses a path other than empty or `/`, and a
  query, at the value: a path (a proxy prefix) can come later as a compatible change,
  and a refusal cannot. Each diagnostic of a kind names its document
  `connector::kind::NOUN` ("the connector"), as `reader::read` does. Decided by
  `laptop.architect-2` on #1794
  (https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052684931,
  2026-10-08 05:03 UTC). The code of that refusal is `influx.bad-address`. Proposed by
  `connector` as its address code
  (https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052532087,
  2026-10-08 04:51 UTC), and approved by item 4 of
  https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052684931
  (2026-10-08 05:03 UTC).
  The kind also reads `database`, which is not empty (`influx.bad-database`), and
  `measurement`, a name that `line::Measurement::new` takes other than
  `foundation_gaps` (`influx.bad-measurement`). `check` gives the reader of its
  settings and no written channel. `run` writes each frame of the reader to InfluxDB 1
  with `POST /write?db=<database>&precision=ns`: one line for each sample time, with
  a field for each channel of the frame, keyed by the channel name. A NaN or infinite
  value is left out, and so is a line with no other value. Until #2555, only `f64`
  channels are written: another type ends the run with `Error::Config` and
  `influx.sample-type`, and a channel name that is not a field key with
  `influx.bad-channel`. The run writes no gap line until #341. One request is in flight
  at a time. While it is, the run reads the next batch, up to 5,000 lines, and a frame
  goes in whole. It acks a batch only after a 204. Each other answer, and each failed
  request, keeps the batch, shows its text with `Context::error`, and sends the batch
  again after a `retry::Backoff` of 1 s to 1 min. A 204 clears the error. A batch whose
  request takes longer than the timeout of 10 s is sent again with no end, until #2558,
  as `laptop.architect-2` decided on 2026-10-13T06:20:10Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6139570127). A
  restart does not send the batch in flight again (#274). With no `hold`, the next run
  reads from the live frames, so those lines are lost. With a `hold`, each open after
  the loss ends `Ended::Behind` until #2553, as `laptop.architect-2` decided on
  2026-10-13T06:16:46Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6139522602). Decided
  by the ONE NODE record, section 4, on 2026-10-13T00:50:41Z
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6135595217), and
  the plan that `laptop.architect-2` approved on #1734
  (https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6135905432).
