- **STREAM DISPATCH (2026-10-04)** `transport` is blind to protocols. The first
  message of each stream, and each datagram, starts with a header from `wire` that
  names the protocol (`clock`, `mesh`, `replica`, `blob`, `hub`). `node` holds the
  table from protocol to handler and runs one accept loop per session. A protocol
  that the table does not know comes from a peer, so the loop resets that stream with
  a code and goes on. Decided by the coordinator (network's review of #53).
  `node` routes a `blob` stream only from a node that is a member of its region at the
  header, to `blob::peer::serve` on the store that it gives `mesh`, and rejects each
  other one. Decided by `laptop.architect-2`, with the OK of `laptop.architect`
  (approval owed).
