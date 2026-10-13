- **NODE START (2026-10-08)** `foundation start` runs a node on a data directory until a
  SIGINT or SIGTERM that reaches it, then exits 0. `ops` cannot depend on `node`, so
  `start` is a `Command` variant beside `mcp`, not an entry of the operation table: like
  `mcp`, it runs the process and is not an operation of a node. The help names it, and
  `foundation docs` does not. `ops::cli` writes nothing for it and gives
  `Run::Start(Start { data, json, name, listen })` to `main` in `node`. `--data` is
  `foundation-data` in the working directory by default, so `cli` reads no environment.
  `start` has no MCP tool: MCP acts on a running node. The first start on a data
  directory needs `--name`, and a later one reads the name there (NODE NAME). Once the
  node has claimed the data directory, a thread of `main` writes `Start::line`: `node
  edge runs in foundation-data. Stop it with Ctrl-C.`, or
  `{"name":"edge","data":"foundation-data"}` with `--json`. The line comes after the
  file `address` holds the address. `data` is the path as given, not made absolute, so
  `cli` stays a function of its arguments. Each form writes it lossily, as
  `Path::to_string_lossy` does. The text form also escapes it as `Start::fail` does,
  each character but a quote as `char::escape_debug` does, so a
  newline in it adds no line, and a backslash or a combining mark shows escaped. A
  literal `\n` in a path then reads apart from a newline, and the line matches the
  `node.data` message for the same path. Lost: the raw path in the text form, which a
  newline splits into two lines; `escape_controls`, as `Config` errors use, which keeps
  each backslash, so a literal `\n` reads as a newline. A write that fails changes
  nothing. A node that stops before the thread writes can exit with no line.
  `Start::fail` writes an `ops::Failure { code, message, fix }` as `cli` writes its own
  errors, and gives exit status 1. So `ops` keeps the one output form, and `main` gives
  the facts. The codes: `node.busy`, `node.data`, `node.unnamed`, `node.renamed`,
  `node.name`, `node.reserved` (NODE NAME), `node.budget`, `node.disk`, `node.memory`
  (NODE SETTINGS), `node.port`, and `node.failed`. `node.data` is "this user cannot use
  the data directory as it is": each `os::Error::Dir` whatever its errno, and each
  `env::files::Error::Io` whose code is `EACCES`, `EPERM`, or `EROFS` that
  `node::Error::files` gives: the failed file call of a `Directory`, `Buffer`, `Blob`,
  `Mesh`, or `Group` error. Its message is "cannot write the data directory {data}:
  {error}", and its fix "Make {data} a directory that this user can write, and let it
  write each file in it, or give another directory with `--data`". The fix also covers a
  missing parent (`ENOENT`) and a file in its place (`ENOTDIR`). Decided by
  `laptop.architect-2` (2026-10-10T22:20:07Z,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6102789069).
  Supersedes the cause list and the fix of
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090417728. Each
  other error is `node.failed`: each other `Directory` error but `Busy`, each buffer
  error that neither `node.data` nor NODE SETTINGS (`Pool`) names, and each `Blob`,
  `Mesh`, or `Group` error that `node.data` does not name. Lost: `node.data` for the
  claim of `lock` alone, which leaves a ring that the user cannot write on
  `node.failed`; `files` on each lower error, four public items for one caller; a walk
  of `source()` with a downcast, type erasure in which a lower error that gives no
  `source` hides its file call with no failed test. Each match of `files` names each
  variant, so a new variant that holds a file call fails to compile in `node`, and each
  new variant needs an edit of `node`. Decided by `laptop.architect`
  (2026-10-10T15:14:44Z,
  https://github.com/synnaxlabs/foundation/issues/2282#issuecomment-6098971338).
  `Node::stopper` gives a `Stopper` that stops the node from another thread. A stop
  after the node ended does nothing. Lost: a line that `node` writes itself, a second
  owner of the output form; an entry of the table with a flag that each of its four
  users skips. Decided by `laptop.architect-2` (2026-10-08T02:21:16Z,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6050855867;
  2026-10-08T05:00:46Z, `--name` and the line,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6052649826;
  2026-10-09T18:30:45Z, `running` (now `line`), `fail`, `Failure`, and the `Stopper`
  doc, https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6086905545;
  2026-10-09T19:31:23Z, `start` as a `Command` variant, which amends the first, and the
  line, https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6087849613;
  2026-10-09T20:14:29Z, `node.disk`, `node.memory`, and a code of its own for the file
  `budget`,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6088506863;
  2026-10-09T21:08:23Z, `node.budget`,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6089288397;
  2026-10-09T21:35:43Z, `Refused` as `node.memory`,
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6089651601;
  2026-10-09T22:29:12Z, the three codes in this list,
  https://github.com/synnaxlabs/foundation/pull/2191#issuecomment-6090312136). `main`
  writes the line on a thread of its own, so a standard output that nobody reads blocks
  neither shard 0 nor the stop. `main` drops the handles of that thread and of the
  thread that waits for the signal, because each waits only on the process
  (`env::thread::Handle`). Decided by `laptop.architect-2` (2026-10-09T22:02:37Z,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6089996352;
  2026-10-09T22:10:27Z, the stop handle also after a clean end,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090093334;
  2026-10-09T22:24:30Z, `Start::line` in place of `Start::running`,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090256620;
  2026-10-09T22:36:49Z, `node.data` by its cause, which supersedes item 2 of
  https://github.com/synnaxlabs/foundation/issues/1732#issuecomment-6086905545,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090417728;
  2026-10-09T23:14:43Z, the escape of `data` in the text form,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090863425;
  2026-10-09T23:23:21Z, why the escape stays: `Start::fail` and `Applied` give a path in
  that form,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6090952680;
  2026-10-09T23:57:51Z, that reason confirmed, and the first bullet of the ruling
  narrowed to `Applied` and each error but `Config`,
  https://github.com/synnaxlabs/foundation/pull/2188#issuecomment-6091313576).
  `main` gives `Region::Stored`, so a start whose mesh directory holds no founding
  founds the root region with the key of `admin.key`, and makes that file when it is
  not there, and each later start opens that region (NODE MESH). Decided by
  `laptop.architect-2` (2026-10-10T03:30:54Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6093320483), and
  moved to #1744 PR 1b-4 by `laptop.architect` (2026-10-10T22:53:09Z,
  https://github.com/synnaxlabs/foundation/pull/2310#issuecomment-6103048603).
  `start --listen` gives `Start::listen`, the address of the node's one port,
  `127.0.0.1:0` by default, so the node takes a free port and no other host reaches it
  until the user gives an address. `main` gives it as `node::Config::listen`. A value
  that is no socket address is `ops.argument`. A port that does not bind is `node.port`,
  since the user chose the address: its message is the text of `node::Error::Port`, and
  its fix "Give `--listen` an address of this host with a port that is free and that
  this user can bind". Lost: `node.failed`, which gives an agent no fix for a value that
  it gave. Decided by `laptop.architect-2` (2026-10-11T10:21:07Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6108020871).
  A `--listen` whose IP is multicast or the IPv4 broadcast IP, also as an IPv4-mapped
  IPv6 address, binds on Linux, and no peer reaches a node at it. `node` refuses it
  before a shard starts, with `node::Error::Listen`, which is `node.port` with the same
  fix. `node` does not refuse a subnet broadcast IP of an interface, which only the
  interface table shows. Trigger: the first change that gives `node` the addresses of
  the interfaces of its host. Lost: a refusal in `ops::cli` as `ops.argument`, which
  leaves each other caller of `node::Config::listen` open; a refusal in the UDP bind of
  `env::net`, where a connector that receives multicast binds such an IP;
  `node::Error::Port` with an OS code that no OS call gave; a code of its own for the
  same fix. Decided by `laptop.architect-2` (2026-10-11T15:33:00Z,
  https://github.com/synnaxlabs/foundation/issues/2413#issuecomment-6110638754).
  Shard 0 claims the data directory before it binds the port. So a start on the data
  directory of a node that runs is `node.busy` with each `--listen` that goes to the
  bind, and such a start during the stop of that node never gets `node.port`, since the
  stop holds the lock until the port is free. A `--listen` that `node` refuses by its
  value is `node.port` there too: `node` checks it before a shard starts, as it checks
  the name and the bounds of the budgets that the doc of `Node::start` names, so that
  start makes no file. A value of `node::Config::settings` that the transport refuses,
  and a part of the pool budget that the buffer refuses, come after the claim (NODE
  PORT, NODE SETTINGS). A start whose port does not bind leaves the lock file, the shard
  count, and the name in the data directory, and opens no buffer. Lost: the bind before
  the claim, where one cause has two codes by the value of `--listen`; the bind before
  the claim with its error kept until after the claim, which can report a port that
  became free in between; the refusal of such a `--listen` after the claim, which starts
  the shards and leaves the lock file, the shard count, and the name in a new data
  directory for a value that never serves. Decided by `laptop.architect-2`
  (2026-10-11T15:33:00Z,
  https://github.com/synnaxlabs/foundation/issues/2413#issuecomment-6110638754; a value
  that `node` refuses, 2026-10-12T15:37:03Z:
  https://github.com/synnaxlabs/foundation/issues/2443#issuecomment-6127496580; the
  checks before and after the claim, 2026-10-12T21:12:43Z:
  https://github.com/synnaxlabs/foundation/pull/2457#issuecomment-6132697370).
  Supersedes "is `node.busy` with each `--listen`" and "a start during the stop of that
  node never gets `node.port`" of
  https://github.com/synnaxlabs/foundation/issues/2413#issuecomment-6110638754.
  Supersedes "`node` checks each value of its config before a shard starts" of
  https://github.com/synnaxlabs/foundation/issues/2443#issuecomment-6127496580.
  Supersedes the order of the bind in the doc of `Node::start` approved in
  https://github.com/synnaxlabs/foundation/pull/2391#issuecomment-6108247629
  (2026-10-11T10:50:50Z).
  Shard 0 writes the file `address` in the data directory in place, after the mesh opens
  and before the port takes a session: the node's key, the public key of its transport,
  and the bound socket address, in one sector with a CRC32C. Its form is private to
  `node`, and the fuzz target `node_identity` takes it by its length. The first start
  makes the file, and the file keeps its sector, so a later start needs no new room and
  a node starts again on a full disk. The start does not fail for what the file holds:
  it removes a file of another length and makes the file again. When shard 0 stops
  serving, before the lock drops, it writes the file empty in place (zero bytes of the
  same length), with no sync. A file with no address of a node (no file, an empty one,
  or one that a write tore) reads as no node that runs. A file with an address is no
  proof that a node runs: a crash, a panic of shard 0, or a failed empty write leaves
  the address, a power cut after a stop can bring it back, and a start that ends before
  its write leaves what the file held. A start that serves writes over it. So a program
  takes only a node that proves the public key at the address, the empty write needs no
  sync, and the port can free before the empty write. A failed write at the start is
  `Error::Directory` and stops the node before it serves; a failed empty write is
  `Error::Directory` unless the node already failed. Lost: a fixed port with no file,
  where two nodes on one host need two flags and the CLI still reads the public key from
  the data directory; the address in `lock`, whose open by a reader races with a start;
  a file made new at each start and removed at the stop (the new file needs new room, so
  a node did not start again once another writer filled the disk; round 1 of #2391); a
  start on a full disk that fails (a node whose rings are made runs on a full disk); a
  failed write that does not stop the node (no program reaches the node, and the failure
  hides); no write at the stop (after a clean stop, a program dials a port that no node
  holds). Decided by `laptop.architect-2` (2026-10-08T05:42:09Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6053197935;
  2026-10-11T07:10:55Z, plan revision 4,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913;
  2026-10-11T08:01:23Z, the remove before the lock drops but not before the port frees,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106927243;
  2026-10-11T10:21:07Z, the remove contract of #1744 PR 2a,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6108020871;
  2026-10-11T10:43:15Z, the write in place,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6108190079;
  2026-10-11T10:50:50Z, this text,
  https://github.com/synnaxlabs/foundation/pull/2391#issuecomment-6108247629).
  `node::client::reach` gives a program on the host of a node a hub client of the node
  that runs on a data directory, as `@admin` (`spec::founding::admin`). It reads
  `address`, then `admin.key`, through `env::files` with the readers of `node`, so a
  simulation gives the simulated disk; it makes and locks nothing, and `os::stored`
  opens `dir/data` of a directory that is there and makes nothing. A file `address`
  with no address of a node is `Error::NotRunning`, which also covers a node that
  still writes it; an `admin.key` that no node wrote is `Error::Key`. In place of an
  unspecified IP in `address`, it dials the loopback IP of its family. A dial, session,
  or hello that fails is `Error::Unanswered` with the address it dialed, and a refused
  hello is `Error::Refused`. Lost: `ops::Unreached`, worded in `ops`, which would name
  the files that only `node` reads and take `hub` for the error of a dial. Decided by
  `laptop.architect-2` (2026-10-11T07:10:55Z, plan revision 4, decision 8 and the 2d
  surface,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913).
  A SIGINT or SIGTERM that the process blocks or ignores at its start, which is the
  choice of its parent, does not reach it (ENV SEAMS): a node that
  `sh -c "foundation start ... &"` starts does not stop at `kill -INT`. Decided by
  `laptop.architect-2` (2026-10-10T23:40:43Z,
  https://github.com/synnaxlabs/foundation/pull/2169#issuecomment-6103406164) and
  reworded by it (2026-10-11T10:39:06Z,
  https://github.com/synnaxlabs/foundation/pull/2389#issuecomment-6108157810).
