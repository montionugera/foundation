- **STREAM DISPATCH (2026-10-04)** `transport` is blind to protocols. The first
  message of each stream, and each datagram, starts with a header from `wire` that
  names the protocol (`clock`, `mesh`, `replica`, `blob`, `hub`). `node` holds the
  table from protocol to handler and runs one accept loop per session. A protocol
  that the table does not know comes from a peer, so the loop resets that stream with
  a code and goes on. Decided by the coordinator (network's review of #53).
  `node` routes a `blob` stream only from a node that is a member of its region at the
  header, to `blob::peer::serve` on the store that it gives `mesh`, and rejects each
  other one. Decided by `laptop.architect-2` (2026-10-18T19:36:18Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254240239), with
  the OK of `laptop.architect` (2026-10-18T19:34:36Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254223202). A put
  head takes a block of the body's length before the body comes, so `node` opens the
  store on a pool of its own, cut from the part of shard 0: a member that sends put
  heads and no bodies holds only that pool, and the transport and the mesh go on
  (`laptop.architect`, 2026-10-18T19:07:27Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6253931889, option
  A). The pool has an eighth of the part (`laptop.architect-2`, 2026-10-18T19:12:02Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6253982711, and
  2026-10-18T19:43:23Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254309658, which
  withdraws the quarter of the comment that decides the route), only on a node with a
  region (`laptop.architect-2`, 2026-10-18T19:19:06Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254066588, which
  supersedes "also for a node with no region" of 6253982711; trigger: a join of a
  running node, #1157, makes the pool at the join).
  Deferred: a put that writes and hashes each body message as it comes, so a head
  holds no block. Trigger: a blob stream from a peer that is not a member.
