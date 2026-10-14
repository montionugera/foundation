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
  The kind also reads one target (`Target`): `database` for InfluxDB 1, or `bucket`
  and `org` for InfluxDB 2 and 3. Supersedes "InfluxDB 1 only" of section 4 of the
  ONE NODE record
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6135595217). Each
  is a string of 1 to 4,096 bytes (`influx.bad-database`, `influx.bad-bucket`,
  `influx.bad-org`), so the query of the write URI holds both names when each byte is
  encoded. A `database` with a `bucket` is `influx.two-targets`, and an `org` with a
  `database` is `influx.org-with-database`. No target, or a `bucket` with no `org`,
  is `document.missing-attribute`. A `database` is written with
  `POST /write?db=<database>&precision=ns`, and a `bucket` with
  `POST /api/v2/write?org=<org>&bucket=<bucket>&precision=ns`, which InfluxDB 2 and
  InfluxDB 3 Core take. InfluxDB 3 takes any `org`. Each name is percent-encoded in
  the query. An optional `token` is a source of a secret (SECRET SOURCES). Each
  request sends it as `Authorization: Token <value>`, which InfluxDB 2 and 3 take,
  and a request with no `token` has no `Authorization`. `run` reads the token at its
  start, and again after a 401 or a 403 before the next attempt. This is option A of
  key choice Q6
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6154974514,
  2026-10-13T20:52:58Z), which item 9 of the ruling of `laptop.architect-2` kept on
  2026-10-13T21:05:46Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155189112).
  With a `token`, a 401 or a 403 gives "Check the token {source}. InfluxDB answered
  {status}: {body}", so that the cut of a long error keeps the source. It shows at
  most 512 bytes of the source, so that a long source keeps the answer. With no
  `token`, "InfluxDB answered {status}: {body}". A token that holds a byte that an
  HTTP header cannot hold gives `Error::Retry`. Approved by
  `laptop.architect-2` on 2026-10-14T04:16:45Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6160767913).
  Supersedes the key choice of no `Authorization` until this token
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6156072634).
  Approved by the person (2026-10-13T16:24:35Z, relayed in
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150321949), and
  the plan that `laptop.architect-2` approved on 2026-10-13T16:53:54Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150691934),
  its review of #2610 on 2026-10-13T21:26:57Z
  (https://github.com/synnaxlabs/foundation/pull/2610#issuecomment-6155556749), and its
  approval of the bound of 4,096 bytes on 2026-10-13T21:36:41Z
  (https://github.com/synnaxlabs/foundation/pull/2610#issuecomment-6155716038).
  The kind also reads `measurement`, a name that `line::Measurement::new` takes other
  than `foundation_gaps` (`influx.bad-measurement`). `check` gives the reader of its
  settings and no written channel. `run` writes each frame of the reader to the write
  path of its target: one line for each sample time, with
  a field for each channel of the frame, keyed by the channel name. A NaN or infinite
  value is left out, and so is a line with no other value. Until #2555, only `f64`
  channels are written: another type ends the run with `Error::Config` and
  `influx.sample-type`, and a channel name that is not a field key with
  `influx.bad-channel`. The run writes no gap line until #341. One request is in flight
  at a time. While it is, the run reads the next batch, up to 5,000 lines, and a frame
  goes in whole. The run keeps the field of each channel that it saw in the frames of
  one key set. A frame of another key set builds them again, as the `hub` reader builds
  its lens, so frames that alternate key sets build them at each change. The trigger to
  keep them for each key set is a workload whose frames alternate key sets, as
  `laptop.architect-2` decided on 2026-10-13T07:36:47Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6140691273). It acks
  a batch only after a 204. Each other answer, and each failed request, keeps the
  batch, shows its text with `Context::error`, and sends the batch again after a
  `retry::Backoff` of 1 s to 1 min. A 204 clears the error, and adds the fields of the
  batch to the count `confirmed` with `Count::add`. The count holds across the runs of
  one call (SUPERVISOR). A NaN or infinite sample writes no field and is not counted
  (#2559, the plan that `laptop.architect-2` approved on 2026-10-13T09:52:20Z:
  https://github.com/synnaxlabs/foundation/issues/2559#issuecomment-6142931456). A
  batch whose request takes longer than the timeout of 10 s is sent again with no end,
  until #2558, as `laptop.architect-2` decided on 2026-10-13T06:20:10Z
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
