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
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254223202). The
  store takes its blocks from the pool of shard 0. A put head holds no block, and at
  most `PUTS_MAX` (8) receipts are open at one time (BLOB PEER). So a member that
  sends put heads and no bodies holds no block of that pool, and the transport and the
  mesh go on (`laptop.architect`, 2026-10-18T20:03:51Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254512129, option
  B, which supersedes option A of 6253931889; `laptop.architect-2`,
  2026-10-18T20:06:45Z:
  https://github.com/synnaxlabs/foundation/pull/3244#issuecomment-6254543021).
