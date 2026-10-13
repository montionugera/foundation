- **STATUS OPERATION (#1735, 2026-10-13)** `status` is a read-only operation with no
  input. The CLI sends `{"status": {}}` to the node, and `ops::serve` runs it and
  replies `{"output": <Output>}`. `ops::status::Output` derives `Serialize`,
  `Deserialize`, and `JsonSchema`, and gives its terminal text with `text()`: a table of
  connector, kind, state, address, in, and confirmed, then a line `<name>: <class>
  error: <text>` for each connector with an error. The CLI makes it a `Response`
  variant, as #1744 does for `plan::Output` (OPS OUTPUT). `--json` and the MCP tool
  later give the same value, with no change to the operation, the type, or the reply.
  The connectors are each connector of the spec in use whose `node` is this node, in
  name order. For each, `serve` opens a `Latest` reader of its status index and each
  channel on it through the node's hub, with the request's subject. All reads run at
  once, and a connector with no frame within 1 s of the request is `unknown`, so a reply
  waits at most 1 s in all. `ops::Node` takes the hub, the clock for the bound, and the
  node's name at construction. `state` is `running`, `restarting`, `stopped`, `ending`,
  or `unknown`; with `unknown`, `restarts`, `counts`, and `error` are null. A connector
  whose newest frame holds no status is `unknown` too. When the reader of a connector
  does not open or ends, `status` fails with `ops.channel-read`, which names the
  connector and gives the text of the hub. `status` needs `read` on each status channel
  of a connector through the `Check` of `plan` and `apply`, and leaves out each
  connector without it. When the node places a connector and the subject reads none, it
  gives `ops.denied` with `On::Nothing`. A spec with no connector on the node gives an
  empty list. When #1028 checks each open of the hub, this check moves there. Deferred
  to https://github.com/synnaxlabs/foundation/issues/2530:
  the time of the next restart, `--json`, and MCP. The CLI and MCP send this one form to
  a node, and the client reader of #1749 does not replace it. Lost: `serve` replies with
  the text, so that `--json` and MCP each need a second reply; `status` asks the
  supervisors, a second source of status with its own access check. Decided by
  `laptop.architect-2` (2026-10-13T00:58:53Z,
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119, which
  supersedes "The operation" of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186; the text
  and the JSON, 2026-10-08T06:29:21Z, that comment). The one form, `read` on each
  channel that `status` reads, and no `next` until the deferred time of the next
  restart: `laptop.architect-2` (2026-10-13T01:00:07Z,
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135757072). The
  reader by the channels of the spec in use, in place of a reader of
  `<connector>.status.*`: `laptop.architect-2` (2026-10-13T01:52:03Z,
  https://github.com/synnaxlabs/foundation/pull/2514#issuecomment-6136338680, item 2,
  which supersedes the selector of item 4 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135737119 and the
  last paragraph of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6135757072).
  `ops::Node::new` takes its seven inputs, the node's name too, as arguments, and an
  eighth input makes them
  `ops::Config`: `laptop.architect-2` (2026-10-13T02:25:59Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136850796). `unknown`
  only for no frame and for a frame with no status, and `ops.channel-read` for a reader
  that fails: `laptop.architect-2` (2026-10-13T02:30:50Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6136904108, item 2).
