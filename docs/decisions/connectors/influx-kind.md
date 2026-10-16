- **INFLUX KIND** `connector_influx::Kind` reads `address` and the reader settings
  (READER SETTINGS). `address` is an `http::Uri`, since a `Name` is a mesh name. `parse`
  reads `address` through `connector::http::uri`, so a plan finds an address that `send`
  refuses before a run. Decided by `laptop.architect-2` on #1153
  (https://github.com/synnaxlabs/foundation/issues/1153#issuecomment-6051297152,
  2026-10-08 03:02 UTC). `discover` gives the default `connector.no-discover`
  (`laptop.architect-2`, 2026-10-16 04:00 UTC:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207771367; it
  supersedes
  https://github.com/synnaxlabs/foundation/issues/1153#issuecomment-6051297152, where
  `discover` gave no documents). The kind also refuses a path other than empty or `/`,
  and a query, at the value: a path (a proxy prefix) can come later as a compatible
  change, and a refusal cannot. Each diagnostic of a kind names its document
  `connector::kind::NOUN` ("the connector"), as `reader::read` does. Decided by
  `laptop.architect-2` on #1794
  (https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052684931,
  2026-10-08 05:03 UTC). The code of that refusal is `influx.bad-address`. Proposed by
  `connector` as its address code
  (https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052532087,
  2026-10-08 04:51 UTC), and approved by item 4 of
  https://github.com/synnaxlabs/foundation/pull/1794#issuecomment-6052684931 (2026-10-08
  05:03 UTC). The kind also reads one target (`Target`): `database` for InfluxDB 1, or
  `bucket` and `org` for InfluxDB 2 and 3. Supersedes "InfluxDB 1 only" of section 4 of
  the ONE NODE record
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6135595217). Each
  is a string of 1 to 4,096 bytes (`influx.bad-database`, `influx.bad-bucket`,
  `influx.bad-org`), so the query of the write URI holds both names when each byte is
  encoded. A `database` with a `bucket` is `influx.two-targets`, and an `org` with a
  `database` is `influx.org-with-database`. No target, or a `bucket` with no `org`, is
  `document.missing-attribute`. A `database` is written with `POST
  /write?db=<database>&precision=ns`, and a `bucket` with `POST
  /api/v2/write?org=<org>&bucket=<bucket>&precision=ns`, which InfluxDB 2 and InfluxDB 3
  Core take. InfluxDB 3 takes any `org`. Each name is percent-encoded in the query. An
  optional `token` is a source of a secret (SECRET SOURCES), so `plan` on the node of
  the connector, by a subject that may apply it, refuses a `token` with no value
  (`secret.unresolved`). Each request sends it as `Authorization: Token <value>`, which
  InfluxDB 2 and 3 take, and a request with no `token` has no `Authorization`. `run`
  reads the token at its start, and again after a 401 or a 403 before the next attempt.
  This is option A of key choice Q6
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6154974514,
  2026-10-13T20:52:58Z), which item 9 of the ruling of `laptop.architect-2` kept on
  2026-10-13T21:05:46Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155189112). With a
  `token`, a 401 or a 403 gives "InfluxDB answered {status}. Check the token {source}:
  {body}", so that the cut of a long error keeps the status. With no `token`, "InfluxDB
  answered {status}: {body}". A token that holds a byte that an HTTP header cannot hold
  gives `Error::Retry`. Approved by `laptop.architect-2` on 2026-10-14T04:16:45Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6160767913); the
  401 and 403 text on 2026-10-14T20:02:29Z
  (https://github.com/synnaxlabs/foundation/pull/2687#issuecomment-6177206734).
  Supersedes the key choice of no `Authorization` until this token
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6156072634).
  Approved by the person (2026-10-13T16:24:35Z, relayed in
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150321949), and the
  plan that `laptop.architect-2` approved on 2026-10-13T16:53:54Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150691934), its
  review of #2610 on 2026-10-13T21:26:57Z
  (https://github.com/synnaxlabs/foundation/pull/2610#issuecomment-6155556749), and its
  approval of the bound of 4,096 bytes on 2026-10-13T21:36:41Z
  (https://github.com/synnaxlabs/foundation/pull/2610#issuecomment-6155716038). The kind
  also reads `measurement`, a name that `line::Measurement::new` takes other than
  `foundation_gaps` (`influx.bad-measurement`). `parse` builds the
  `line::Measurement` once, and the run takes it. `Config` is opaque, as in `opcua`
  and `modbus`: only `parse` makes one, so each `Config` holds checked names (#2555,
  item 2). `check` gives the reader of its settings
  and no written channel. `run` writes each frame of the reader to the write path of its
  target: one line for each sample time, with a field for each channel of the frame,
  keyed by the channel name. A NaN or infinite value is left out, and so is a line with
  no other value. A `bool` channel is a boolean field. An `i8` to `i64`, `u8` to `u32`,
  `timestamp` (ns since the Unix epoch), or `duration` (ns) channel is an `i` integer.
  So is a quality channel (`DataType::Quality`), a choice that the person has (LINE
  TEXT). An `f32` or `f64` channel is a float, where an `f32` is the `f64` of the same
  value (0.1 as 1.0000000149011612e-1), so a reader that narrows it gets back the same
  bits. A `u64` channel is a `u` integer to a `bucket`, and to a `database` it ends the
  run with `Error::Config` and `influx.unsigned` (LINE TEXT). Each other type (`uuid`,
  `string`, `bytes`, `array`, `list`, `matrix`) ends the run with `Error::Config` and
  `influx.sample-type`, and a channel name that is not a field key with
  `influx.bad-channel`. The run picks the conversion of a channel with its field, at the
  first frame of each key set that holds it. Lost: an `f32` as its shortest text
  (`1e-1`), as InfluxDB then holds the `f64` 0.1, which is not the value of the sample.
  The types: the type table of `laptop.architect-2` on 2026-10-15T16:13:56Z
  (https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6196722652),
  approved by the person without arrays on 2026-10-15T16:51:53Z
  (https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6197478869); the
  surface: `laptop.architect-2` on 2026-10-16T09:09:18Z
  (https://github.com/synnaxlabs/foundation/issues/2555#issuecomment-6212098264). After
  a `Received::gap` that is not `None`, the run writes its gap line with `path=live`
  before the lines of the frame, at the stamp of the frame's first sample, also when it
  leaves that line out (INFLUX SEQ AND GAPS), in the same batch. The line's `count` is
  the seqs of the gap only. Each batch acks the position after its last frame. A frame
  with no sample gives no gap, and its position stays before the gap that the next frame
  with one gives (`hub`), so the run needs no wait of its own. The person chose this
  path ("Ok, this is fine", 2026-10-16T02:03Z, through `laptop.monitor`:
  https://github.com/synnaxlabs/foundation/pull/2794#issuecomment-6206366130), and
  `laptop.architect-2` ruled it on 2026-10-16T02:04:28Z
  (https://github.com/synnaxlabs/foundation/pull/2794#issuecomment-6206384923). It
  supersedes ruling 2 of
  https://github.com/synnaxlabs/foundation/pull/2794#issuecomment-6206327837, a gap that
  waits for a line with a value. One request is in flight at a time. While it is, the
  run reads the next batch, up to 5,000 lines, and a frame goes in whole. The run keeps
  the field of each channel that it saw in the frames of one key set. A frame of another
  key set builds them again, as the `hub` reader builds its lens, so frames that
  alternate key sets build them at each change. The trigger to keep them for each key
  set is a workload whose frames alternate key sets, as `laptop.architect-2` decided on
  2026-10-13T07:36:47Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6140691273). It acks
  a batch only after a 204. Each other answer, and each failed request, keeps the batch,
  shows its text with `Context::error`, and sends the batch again after a
  `retry::Backoff` of 1 s to 1 min. No answer ends the run, other than a 401 or a 403
  after which the read of the token fails: an action in InfluxDB fixes a 400, a 404, or
  a 422, and the next try after it lands the batch. The reader stays open meanwhile, as
  in an outage of InfluxDB. The text is `InfluxDB answered {status}: {body}`, and for
  these three statuses it adds a fix. A 400 and a 422 (InfluxDB 1 gives 400, and
  InfluxDB 2 gives 422, for a field type conflict and for points past retention): "For a
  field type conflict, write to another `measurement` or delete the old field in
  InfluxDB. For points older than the retention period, raise it or write to another
  bucket". A 404: "Create the database, the bucket, or the org in InfluxDB, or name one
  that exists". An answer whose body is over the cap of 64 KiB has its first 64 KiB as
  `{body}`, as `connector::http::Error::TooLarge` holds them, and the fix of its status
  (`laptop.architect-2`, 2026-10-16T07:38:36Z,
  https://github.com/synnaxlabs/foundation/pull/2838#issuecomment-6210559216). Decided
  by `laptop.architect-2` on 2026-10-16T07:18:54Z
  (https://github.com/synnaxlabs/foundation/pull/2838#issuecomment-6210258673), which
  replaces the stop at a 400, a 404, or a 422 of the ruling of 2026-10-14T01:32:02Z
  (https://github.com/synnaxlabs/foundation/issues/2607#issuecomment-6158874569).
  QUARANTINE, which lets the connector go on past refused points, is a later step
  (#2841). A 401 and a 403 keep the backoff, since the run reads the token again before
  the next try (#2687). A 413 keeps the backoff too. Trigger for a split of the batch: a
  server whose body limit is below one batch. A 204 clears the error, and adds the
  fields of the batch to the count `confirmed` with `Count::add`, and the `count` of
  each gap line of the batch to the count `lost`: the seqs that the gap lines of this
  connector count, which InfluxDB took with a 204. A batch sent again counts once. A gap
  that the reader reports again after a restart counts again, so `lost` is an upper
  bound, as the sum of `count` in `foundation_gaps` is. Each later out kind names the
  same count `lost`. It counts sample times of the reader's index. The `lost` of `opcua`
  counts data samples (OPCUA KIND). The ruling did not take a count of all out kinds in
  `connector`, as the supervisor does not see the frames of a kind's reader. The trigger
  to make it is a second out kind (6199058253). Both counts hold across the runs of one
  call (SUPERVISOR). The person gave the choice of the path to the architects on
  2026-10-15 ("Best devx possible. You choose the path", through `laptop.monitor`:
  https://github.com/synnaxlabs/foundation/issues/2553#issuecomment-6199035922).
  `laptop.architect` gave the count of lost seqs, and `laptop.architect-2` ruled its
  shape on 2026-10-15T18:13:22Z
  (https://github.com/synnaxlabs/foundation/issues/2553#issuecomment-6199058253) and the
  gap lines of the run on 2026-10-15T18:20:15Z
  (https://github.com/synnaxlabs/foundation/issues/2725#issuecomment-6199190890). A NaN
  or infinite sample writes no field and is not counted (#2559, the plan that
  `laptop.architect-2` approved on 2026-10-13T09:52:20Z:
  https://github.com/synnaxlabs/foundation/issues/2559#issuecomment-6142931456). A batch
  whose request takes longer than the timeout of 10 s is sent again with no end, until
  #2558, as `laptop.architect-2` decided on 2026-10-13T06:20:10Z
  (https://github.com/synnaxlabs/foundation/pull/2552#issuecomment-6139570127). A
  restart within the `hold` of the reader sends the batch in flight again: the next run
  reads its frames again from disk (#274 B1; `laptop.architect-2`, 2026-10-15T18:05:36Z:
  https://github.com/synnaxlabs/foundation/pull/2723#issuecomment-6198895692). With no
  `hold`, or after it, the next run reads from the live frames, so those lines are lost.
  Decided by the ONE NODE record, section 4, on 2026-10-13T00:50:41Z
  (https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6135595217), and
  the plan that `laptop.architect-2` approved on #1734
  (https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6135905432).
