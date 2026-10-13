- **STATUS OPERATION (#1735, 2026-10-13)** `status` is a read-only operation with no
  input. The CLI sends `{"status": {}}` to the node, and `ops::serve` runs it and
  replies `{"output": <Output>}`. `ops::status::Output` derives `Serialize`,
  `Deserialize`, and `JsonSchema`, and gives its terminal text with `text()`: a table of
  connector, kind, state, address, in, and confirmed, then a line `<name>: <class>
  error: <text>` for each connector with an error. The column `in` gives the count `in`:
  the samples that the connector read and the home applied (`laptop.architect-2`,
  2026-10-13T04:35:20Z,
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6138315099, which
  supersedes the `c.status.in` line of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186). The CLI
  writes the `text()` of the reply, as it does for `plan` (`laptop.architect`,
  2026-10-13T08:24:06Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141442056, and
  `laptop.architect-2`, 2026-10-13T08:24:09Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141442842). `status`
  has no entry in the operation table and no `Response` variant, as `plan` and `apply`
  have none (`laptop.architect-2`, 2026-10-13T08:31:04Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141554758, item 1,
  which supersedes "read-only in the operation table" of item 1 and "is a variant of
  `Response`" of item 2 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119; approved
  by `laptop.architect`, 2026-10-13T08:48:14Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141847854).
  `foundation status --json` writes the `Output` of the reply as one line of JSON, as
  `plan --json` does (the person, relayed by `laptop.monitor`, 2026-10-13T13:34:58Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6146776054, by item 3
  of https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141554758). The
  MCP tool later gives the `Output` of the reply as it is, with no change to the
  operation, the type, or the reply. The connectors are each connector of the spec in
  use whose `node` is this node, in name order. For each, `serve` opens a `Latest`
  reader of its status index and each channel on it through the node's hub, with the
  request's subject. All reads run at once, and a connector with no frame within 1 s of
  the request is `unknown`, so a reply waits at most 1 s in all. A link holds one open
  request for each session, and `node::Settings::places_max` bounds the sessions of
  programs, so at most that bound times the connectors of the node are open readers of
  `status` at once (`laptop.architect`, 2026-10-13T02:42:31Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137032724).
  `ops::Node` takes the hub, the clock for the bound, and the node's name at
  construction. `state` is `running`, `restarting`, `stopped`, `ending`, or `unknown`;
  with `unknown`, `restarts`, `counts`, and `error` are null. A connector whose newest
  frame holds no status or lacks a status channel of the spec is `unknown` too. So is a
  connector whose status channels the hub does not hold as the spec defines them yet (no
  channel, a key, a sample type, or an index of another spec), and one whose status
  channels an apply removes during the read. A frame that holds each status channel, of
  channels that `config` carries over with their keys and definitions, is the status
  until the supervisor writes its first frame
  (https://github.com/synnaxlabs/foundation/issues/2531). When the reader of a connector
  does not open or ends for another cause, `status` fails with `ops.channel-read`, which
  names the connector and gives the text of the hub. `status` needs `read` on each
  status channel of a connector through the `Check` of `plan` and `apply`, and leaves
  out each connector without it. When the subject reads no connector of the node, also
  when the node places none or uses no spec, `status` needs `read` on the tree key of
  the node. With it, the reply is the output with no connector, and with no grant, it
  is `ops.denied` with `On::Nothing`, so the reply does not tell whether the node places
  a connector that the subject does not read (`laptop.architect`, 2026-10-13T02:42:31Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137032724, which
  supersedes "A spec with no connector on the node gives an empty list" of item 5 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119;
  `laptop.architect`, 2026-10-13T21:55:31Z,
  https://github.com/synnaxlabs/foundation/issues/2616#issuecomment-6156024198, which
  supersedes the refusal of a node that places no connector of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137032724). The
  tree key of a node is its name. The text of the output with no connector is the line
  `No connector to show.`, which is true in each of these cases. When #1028 checks each
  open of the hub, the check of the status channels moves there, and the check on the
  node stays in `serve`, as it opens no reader. Deferred to
  https://github.com/synnaxlabs/foundation/issues/2530: the time of the next restart
  and MCP. Lost: `serve` replies with the text, so that `--json` and MCP each need a
  second reply; `status` asks the supervisors, a second source of status with its own
  access check. Decided by `laptop.architect-2` (2026-10-13T00:58:53Z,
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119, which
  supersedes "The operation" of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186; the text
  and the JSON, 2026-10-08T06:29:21Z, that comment). The one request `{"status": {}}`
  for the CLI and MCP, which the client reader of #1749 does not replace, `read` on each
  channel that `status` reads, and no `next` until the deferred time of the next
  restart: `laptop.architect-2` (2026-10-13T01:00:07Z,
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135757072, item (a)
  of which supersedes the Lost item on #1749 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119). The
  reader by the channels of the spec in use, in place of a reader of
  `<connector>.status.*`: `laptop.architect-2` (2026-10-13T01:52:03Z,
  https://github.com/synnaxlabs/foundation/pull/2514#issuecomment-6136338680, item 2,
  which supersedes the selector of item 4 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119 and the
  last paragraph of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135757072).
  `ops::Node::new` takes its seven inputs, the node's name too, as arguments, and an
  eighth input makes them `ops::Config`: `laptop.architect-2` (2026-10-13T02:25:59Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136850796). `unknown`
  only for no frame and for a frame with no status, and `ops.channel-read` for a reader
  that fails: `laptop.architect-2` (2026-10-13T02:30:50Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136904108, item 2).
  `unknown` also for a hub that does not define the status channels yet and for a
  removed channel: `laptop.architect-2` (2026-10-13T03:01:14Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137241102, item 1,
  which supersedes item 2 of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136904108 for those
  two cases). `unknown` also for a hub that holds a channel of another spec under a
  status name or lacks a status channel of the spec, found by the key and the sample
  type of each channel of the frame, its index too, by the count of its channels, and by
  `ManyIndexes`: `laptop.architect-2` (2026-10-13T03:18:41Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137433437, which
  extends item 1 of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137241102 and
  supersedes, for `ManyIndexes`, its third bullet and item 2 of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136904108). The
  reader selects the names of the spec, and `config` never gives a key to a second name,
  so the key also fixes the name: `laptop.architect-2` (2026-10-13T04:33:01Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6138289687, which
  supersedes the name of item 1 of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137433437). When
  `foundation rename` (A4) keeps the key of a renamed channel, the PR that builds it
  checks this again. The check of the key and the sample type finds another key, sample
  type, or index under a status name, but not a channel that `config` carries over with
  its key and definition (the gap above): `laptop.architect-2` (2026-10-13T03:32:04Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137583351, which
  corrects the scope of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137433437). The count
  of the channels is inside item 1 of
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137433437:
  `laptop.architect-2` (2026-10-13T03:45:13Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137750716). A frame
  of a run of an earlier spec that lacks a count of the spec in use is `unknown` by that
  count too, until the run of the spec in use writes its start frame:
  `laptop.architect-2` (2026-10-13T04:05:46Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6137996480, and
  2026-10-13T04:47:16Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6138447379, which
  extends it to each missing count). A frame of a run of an earlier spec whose counts
  are each count of the spec in use and more holds each status channel of the spec in
  use, so it gives its state until the run of the spec in use writes its start frame.
  That state can be `running`, because the earlier run writes no `stopped` frame once
  the apply removed a channel that its frames hold: `laptop.architect-2`
  (2026-10-13T04:28:21Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6138238827).
