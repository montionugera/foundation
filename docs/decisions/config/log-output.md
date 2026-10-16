- **LOG OUTPUT (#2606, 2026-10-15)** A node writes its log to stderr, one line for each
  `tracing` event at the level of `--log-level` or above.
  (1) Flags of `foundation start`: `--log-level error|warn|info|debug|trace`, default
  `info`, lower case only, and `--log-format text|json`, whose default is `json` with
  `--json`, else `text`. A bad value is `ops.argument`, whose message ends with
  `` `<value>` is not one of <the values>``. No `off`: send stderr to `/dev/null`.
  `--json` stays the format of the command's own lines. The person approved structured
  events with a text and a JSON output and a level flag: "Ok, this is fine"
  (2026-10-13T21:06:00Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155197898). The
  details by `laptop.architect-2` (item 3 of 2026-10-13T21:13:43Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155317582).
  (2) The stamp of a line is the OS wall time of the node, in RFC 3339 UTC with six
  fraction digits, floored. Mesh time is the field `mesh_time`: absent for
  `clock::log::Mesh::Near`, the best guess in the stamp's form for `Apart`, and `none`
  (JSON `null`) for `Unsynced` (CLOCK LOG gives the cases and the 2 ms). The person:
  "Yeah I agree. System clock first" (2026-10-13T21:17:06Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155377794). The
  field by `laptop.architect-2` (2026-10-13T21:25:00Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155519701), amended
  by 2026-10-14T04:35:24Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6160981810.
  (3) Text: `<stamp> <LEVEL> <target>: <message> <key>=<value>... [mesh_time=...]`,
  with `mesh_time` last. The message has `\` and each control character escaped as
  `char::escape_debug` gives them. A string value that is empty, or holds a space, `=`,
  `"`, `\`, or a control character, is in double quotes, with `"`, `\`, and each
  control character escaped; each other value is plain, as logfmt writes it. So a value
  reads back to one string and cannot look like a second field. Integers and booleans
  are plain. JSON: one object, `{"timestamp", "level", "target", "fields":
  {"message", ...}, "mesh_time"}`, in that order, with the fields in event order,
  integers and booleans as JSON numbers and booleans, and each other value a string.
  By `laptop.architect-2` (items 2 and 3 of 2026-10-15T20:27:52Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6201604736).
  (4) `node::log` builds the `fmt` layer of `tracing-subscriber` with a `FormatEvent`
  of its own, which reads `clock::log::Reader::now` once for each line and writes the
  whole line, with `log_internal_errors(false)`, so a failed write drops the line and
  `fmt` writes nothing of its own to stderr. A `LevelFilter` of `--log-level` is under
  it. `fmt` keeps the buffer of each thread, the one write of each line, and the span
  fields that a later step needs. Features `fmt`, `registry`, and `std` only: no
  `ansi`, `json`, or `tracing-serde`. By `laptop.architect-2` (2026-10-15T20:28:52Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6201622804).
  Lost: a `Layer` of our own, which builds again what `fmt` gives; the `json` format
  of `fmt`, which needs a second clock read or a splice for `mesh_time`. A write
  blocks the thread of its event until stderr takes the line, and each other event of
  the node, on any shard or thread, waits for that write. Later step (#2764): the
  node never blocks on its log reader, by `laptop.monitor` with the details of
  `laptop.architect-2` (2026-10-15T21:39:25Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6202838819).
  (5) No dispatch is global (crate map rule 9, `clippy.toml`). `Node::start` makes the
  dispatch from `Config::log` and gives it to its shards through `Shards::logged`.
  `node::Log` holds the writer and the `ops::Log` that `start` parses, whose default
  is text at `info`. By `laptop.architect-2` (2026-10-15T21:40:21Z,
  https://github.com/synnaxlabs/foundation/pull/2762#issuecomment-6202853376).
  Shard 0 writes `started` (INFO, `name`, `address`: the bound address, `shards`) once
  the file `address` holds the address. `Node::join` writes `stopped`: INFO after a
  stop, ERROR with `error` after a failure, also a failure before any shard started.
  (6) The fix of `node.session` does not point to the log, which holds no event of a
  failed session yet (OPS OUTPUT, #2767). By `laptop.architect-2`
  (2026-10-15T22:05:27Z,
  https://github.com/synnaxlabs/foundation/pull/2762#issuecomment-6203245390).
  (7) `node` starts no thread through `env::threads::Threads`, and no thread of `main`
  writes an event, so `node::Config` has no `threads` yet. The first part that needs
  one (the thread of open62541) adds `Config::threads` and calls `Threads::logged`.
  The threads of `main` (`show`, `stop`, the I/O thread of each disk) get the dispatch
  with the first event one of them writes. By `laptop.architect-2`
  (2026-10-15T20:38:04Z,
  https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6201785053).
  (8) P1 holds at `info`: about 1.1 µs for each run of a node and 33 KB idle. A level
  under `info` is an operator's choice for debug, and its cost is outside P1. At
  `debug` and `trace`, `noq-proto` writes a line for each packet and frame of QUIC, at
  about 425 to 739 ns and 2 to 6 allocations before the write, and at `trace` a span
  at 118 ns and 2 allocations (laptop, M3 Max, load 9, 3 passes). The help of
  `--log-level` says so. #2764 carries the shared lock, the allocations, the span
  fields that `Line` never prints, and the WARN of `noq-proto` for each Initial packet
  with an invalid retry token. By `laptop.architect-2` (2026-10-16T01:49:38Z,
  https://github.com/synnaxlabs/foundation/pull/2762#issuecomment-6206195113).
