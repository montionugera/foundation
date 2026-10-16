- **INFLUX KIND** `connector_influx::Kind` reads `address` and the reader settings
  (READER SETTINGS). `address` is an `http::Uri`, since a `Name` is a mesh name. `parse`
  reads `address` through `connector::http::uri`, so a plan finds an address that `send`
  refuses before a run. `discover` gives no documents. Decided by `laptop.architect-2`
  on #1153
  (https://github.com/synnaxlabs/foundation/issues/1153#issuecomment-6051297152,
  2026-10-08 03:02 UTC). The address takes the scheme `http` or `https` (#2702). An
  optional `ca` is a source of a secret (SECRET SOURCES) that holds PEM certificates, so
  `plan` refuses a `ca` with no value (`secret.unresolved`), as it does a `token`. An
  `https` run trusts the roots of the node and each certificate of the `ca`, and checks
  the server's certificate at mesh time (`https` in R7 starting points). A `ca` with an
  `http` address is `influx.ca-with-http`, with "the connector has a `ca`, which only an
  `https` address takes" and the fix "Use an `https` address, or remove `ca`": a `ca`
  there trusts nothing, and its user likely thinks that the token goes under TLS. A run
  reads the `ca` at its start. A TLS failure of a run with a `ca` shows "the write to
  InfluxDB failed: {error}. Check the ca {source}", and the run reads the `ca` again,
  with a new client, before the next attempt of the same batch, as it reads the token
  again after a 401. A run with no `ca` retries a TLS failure in the run. `Error::Tls`
  also covers a server that closes in the handshake, so such a fault shows "Check the
  ca" too. The rule is one rule: a fault retries in the run, and a config that cannot
  work ends it. So a `ca` that gives no root when read again ends the run with
  `Error::Retry`, and with no `hold` that end loses the batch in flight. Approved by
  `laptop.architect-2` (2026-10-16T16:46:18Z:
  https://github.com/synnaxlabs/foundation/pull/2888#issuecomment-6220307355).
  Supersedes item 1 of 6219779608: the end of the run at a TLS failure with a `ca`
  (`laptop.architect-2`, 2026-10-16T16:17:47Z:
  https://github.com/synnaxlabs/foundation/pull/2888#issuecomment-6219779608). A `ca`
  that holds no PEM certificate gives `Error::Retry` with "the ca {source} holds no PEM
  certificate. Write the CA certificate of the server in PEM", and
  one that does not parse gives "the ca {source} holds a PEM certificate that does not
  parse. Write the CA certificate of the server in PEM". Over `https`, the token does
  not cross the network in clear. Planned on #2702
  (https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6218838848,
  2026-10-16T15:28:25Z), and approved with the refusal of a `ca` with an `http` address
  by `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/issues/2702#issuecomment-6219190400,
  2026-10-16T15:47:32Z), which also approved `Error::Retry` for a bad `ca`.
  Supersedes the refusal of `https` until the kind takes a CA (`laptop.architect-2`,
  2026-10-16T07:51:33Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6210764044, item 5)
  and its code (`laptop.architect-2`, 2026-10-16T09:11:38Z:
  https://github.com/synnaxlabs/foundation/pull/2844#issuecomment-6212137276), and
  design item 6 of https://github.com/synnaxlabs/foundation/issues/2702, "A `ca` with
  no certificate in it is `Error::Config`". Lost: `ca` as a setting of `connector`,
  read for each HTTP kind, with the read again at a TLS failure, since `influx` is the
  one HTTP kind. Trigger: a second kind
  that sends `https` (#2894). Lost: a read of the `ca` in `plan`, which finds a bad
  file before a run, since `plan` checks only that a source resolves, and the file can
  change between plan and run. Trigger: a `plan` that reads the value of a secret
  source. The kind
  also refuses a path other than empty or `/`, and a query, at the value: a path (a
  proxy prefix) can come later as a compatible change, and a refusal cannot. Each
  diagnostic of a kind names its document `connector::kind::NOUN` ("the connector"), as
  `reader::read` does. Decided by `laptop.architect-2` on #1794
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
  `foundation_gaps` (`influx.bad-measurement`). `check` gives the reader of its settings
  and no written channel. `run` writes each frame of the reader to the write path of its
  target: one line for each sample time, with a field for each channel of the frame,
  keyed by the channel name. A NaN or infinite value is left out, and so is a line with
  no other value. Until #2555, only `f64` channels are written: another type ends the
  run with `Error::Config` and `influx.sample-type`, and a channel name that is not a
  field key with `influx.bad-channel`. After a `Received::gap` that is not `None`, the
  run writes its gap line with `path=live` before the lines of the frame, at the stamp
  of the frame's first sample, also when it leaves that line out (INFLUX SEQ AND GAPS),
  in the same batch. The line's `count` is the seqs of the gap only. Each batch acks the
  position after its last frame. A frame with no sample gives no gap, and its position
  stays before the gap that the next frame with one gives (`hub`), so the run needs no
  wait of its own. The person chose this path ("Ok, this is fine", 2026-10-16T02:03Z,
  through `laptop.monitor`:
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
  `retry::Backoff` of 1 s to 1 min. A 204 clears the error, and adds the fields of the
  batch to the count `confirmed` with `Count::add`, and the `count` of each gap line of
  the batch to the count `lost`: the seqs that the gap lines of this connector count,
  which InfluxDB took with a 204. A batch sent again counts once. A gap that the reader
  reports again after a restart counts again, so `lost` is an upper bound, as the sum of
  `count` in `foundation_gaps` is. Each later out kind names the same count `lost`. It
  counts sample times of the reader's index. The `lost` of `opcua` counts data samples
  (OPCUA KIND). The ruling did not take a count of all out kinds in `connector`, as the
  supervisor does not see the frames of a kind's reader. The trigger to make it is a
  second out kind (6199058253). Both counts hold across the runs of one call
  (SUPERVISOR). The person gave the choice of the path to the architects on 2026-10-15
  ("Best devx possible. You choose the path", through `laptop.monitor`:
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
