- **SDK CLIENT** `sdk::Client` is a blocking client that owns one thread, with a Tokio
  `LocalRuntime`. The thread holds a `program::Session` on a `transport::Client`, so the
  `!Send` session stays out of the surface, and `sdk` builds on no node core (`hub`,
  `home`, `mesh`, `raft`). The subject's key is `private_key`. `laptop.architect-2`
  decided on 2026-10-16 at 21:06 UTC
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6224695365).
  `laptop.architect` approved the boundary (2026-10-16T21:33:33Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225122027). Amended
  (2026-10-17T00:22:08Z, `laptop.architect-2`,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6227271021): `sdk`
  takes its clock, entropy, and network from `os::clock()`, `os::entropy()`, and
  `os::net()`, as the node does, so `sdk` touches the OS only through `os`.
  `laptop.architect` approved the edge `sdk` -> `os` (2026-10-17T17:03:12Z,
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6237211963). The
  client's thread is a shard of `os::shards()`, named `foundation-sdk`, with no core
  (`laptop.architect-2`, 2026-10-17T17:32:45Z,
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6237701374). The
  shard logs to the dispatch of the caller of `connect`, so the client's logs go where
  the program's logs go (`laptop.architect-2`, 2026-10-17T18:19:03Z,
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6238455226; a key
  choice of kind 9, finding 2 of
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6238447555). This
  replaces the env of its own on `Instant`, `getrandom`, and `socket2`, so #2932 closes:
  `os::clock` counts time while the host sleeps. `sdk` is on the Windows lists of
  `ci.yaml` until #682, #2955, and #2956 give `os::clock`, `os::net`, and
  `os::stored` there (#2956: `laptop.monitor`, 2026-10-18T20:37Z,
  https://github.com/synnaxlabs/foundation/pull/3255#issuecomment-6254946471).
  `Client::connect(&Node, Subject)` takes the node (its UDP addresses, its key, and its
  public key) apart from the subject (its name and private key): one node serves many
  subjects, and a node can listen on more than one address. `laptop.architect-2` decided
  on 2026-10-16 at 21:39 UTC
  (https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225208118).
  `Node::local(dir)` gives the node that runs on a data directory and its first admin
  as the subject: it opens `dir/data` with `os::stored`, and runs
  `sector::local::read` on a shard as `connect` does, which logs to the caller's
  dispatch (`laptop.architect-2`, 2026-10-18T16:46:56Z,
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6252485660, and
  2026-10-18T20:05:17Z,
  https://github.com/synnaxlabs/foundation/issues/3212#issuecomment-6254526787).
  `Client::close` blocks until the close of the session goes out, the transport drains
  each close (up to 3 s), and the thread ends. Drop only signals the end, and a panic of
  the thread after a drop goes to the panic hook of the process (`laptop.architect-2`,
  2026-10-16T21:46:22Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225303374). In a
  build where panics unwind, a panic of the client's thread gives `Error::Panicked` from
  `connect` or `close`, and the panic hook of the process gets its message
  (`laptop.architect-2`, 2026-10-17T17:32:45Z,
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6237701374 and
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6237729706). The wheel
  builds with the profile `wheel`, `release` with `panic = "unwind"`, so a panic in the
  client gives the Python caller an exception, not an abort (`laptop.architect`,
  2026-10-17T17:33:46Z,
  https://github.com/synnaxlabs/foundation/pull/3048#issuecomment-6237722519). It lands
  with the wheel build in PR 1b of #2923. A failure of the session after `connect`
  stays silent until the next call, and that call gives the cause of the failure, never
  a generic closed error
  (`laptop.monitor`, key choice of kind 9, 2026-10-16T22:40:46Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6226048578). The next
  call, `close` too, gives the cause: `close` gives `Error::Session` with it when the
  session ended before the close (`laptop.architect-2`, 2026-10-16T22:20:16Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225794091).
  `Client::open_reader(read::Config)` opens a `sdk::read::Reader` of channels by name,
  on any indexes. `read::Config::new` gives `Mode::Complete`. Until #1818 gives one
  client read over more indexes, the client's thread opens one `program::Reader` for
  each index, in a task of its own, and merges them (key choices of kinds 8 and d,
  approved by the person, 2026-10-18T22:45:12Z,
  https://github.com/synnaxlabs/foundation/issues/3265#issuecomment-6256102155;
  `laptop.architect-2`, 2026-10-18T22:47:08Z, item 2 of
  https://github.com/synnaxlabs/foundation/issues/3265#issuecomment-6256120203, which
  supersedes item 1 of
  https://github.com/synnaxlabs/foundation/issues/2177#issuecomment-6198573817). No
  request of the node gives the index of a channel, so the task first opens one reader
  of all the names. When that fails or leaves a name out, it opens a latest reader of
  each name, keeps only its index, and drops it at once, then opens one reader for each
  index: `1 + n + g` opens for `n` names on `g` indexes, and
  at most `g` streams held (`laptop.architect-2`, 2026-10-19T00:18:31Z, item 2 of
  https://github.com/synnaxlabs/foundation/pull/3306#issuecomment-6256977605, which
  supersedes item 3 of the comment above). Place `n` of a reader is
  `Config::channels[n]`, so a channel named twice is at each of its places. Then come
  the index of each index that `channels` does not name, in the order of the first
  channel of each index, as for a writer. `Frame::index`, `series`, `lost`, and
  `Error::Decode::place` give these places, and each lookup is O(1) (item 1 of the same
  comment). The frames of one index come in order. Across indexes, the
  frame with the earliest first stamp of those that the client holds comes first, a
  frame of no sample first, and the first index on a tie. No call waits for an index
  with no frame. The task decodes each series with `codec` into a frame that the reader
  keeps, with the lost seqs of `program::Reader::lost` (edge `sdk` -> `codec`:
  `laptop.architect`, 2026-10-18T22:45:56Z, item 2 of
  https://github.com/synnaxlabs/foundation/issues/3265#issuecomment-6256109116). An
  error of the read of one index ends the reader, and the frames that the reads of the
  other indexes hold are not given (item 7 of the 2026-10-19T00:18:31Z comment). Each
  frame that the node sent and the caller did not read holds a part of the receive
  window of the client's session. When the window is full, each stream of the client
  waits: the answers to its writers, its other readers, and the renewal of its hello,
  and the node closes the session when the hello ends. Read each reader as its frames
  come, or read on a client of its own (item 3 of the same comment). Flow control for
  each reader is #3309.
