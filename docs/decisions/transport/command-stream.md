- **COMMAND STREAM (#2019, 2026-10-19)** A session lets its peer open `streams_max` + 1
  streams of each kind, and only a `Class::Command` stream takes the last one. A one-way
  count still adds one more for the hello. A stream of another class waits while the
  peer allows exactly one more of its kind and no `Command` stream that this side opened
  in that direction is open. So streams of other classes still get `streams_max`, also
  while the raft sender holds its one-way `Command` stream, and a forward of a mesh
  proposal never waits for the readers at a home, which each hold a two-way stream of
  the one session to it. `Event::Available` wakes each waiter of the session, so a
  `Command` that waits behind waiters of other classes still opens. The rule holds on
  each session, with no check of the peer: a node opens no stream to a client. The `+1`
  is in the grant of a node (`transport::Config`), so a client grant stays one stream of
  each kind. A client may open one more stream than `streams_max` to a node, which
  `program` and `sdk` keep for a `Command` stream and leave unused, as they open none; a
  hostile client can take it, as the peer writes the class of each stream
  (`laptop.architect-2`, 2026-10-19T14:02:03Z:
  https://github.com/synnaxlabs/foundation/issues/2019#issuecomment-6269062543).
  `Streams::available` of the local patch of `noq-proto` 1.3.0 (`docs/dependencies.md`)
  gives the count that the peer allows now. The default `streams_max` of 64 holds: a
  node opens 64 readers at one home. `hub` and `mesh` keep their code. Lost: the last
  stream inside `streams_max`, with `Error::Config` for a `streams_max` of 1 (a
  `node_settings` policy sets the limit, and 64 would give readers 63); a wait at one
  more stream with no look at the open `Command` streams (a held raft sender gives other
  one-way classes `streams_max` - 1, and a freed stream does not open a waiting
  `Complete` while a `Command` is open); a second session per peer for the mesh (ONE
  SESSION PER PEER gains a kind of session); the readers of one node at one home on one
  stream (a new multiplexed read in HUB WIRE, and a slow reader holds the others); a cap
  in `hub` of its remote readers at `streams_max` - 1 (the hub knows the cap of the
  transport, and the peer's value can differ); mesh forwards on one-way streams with
  request numbers (MESH WIRE changes, and a one-way stream of another class can still
  take the credit). Decided by `laptop.architect-2` for `transport` and `node`
  (2026-10-19T13:55:48Z:
  https://github.com/synnaxlabs/foundation/issues/2019#issuecomment-6268935147) and by
  `laptop.architect` for `hub` and `mesh` (2026-10-19T13:56:48Z:
  https://github.com/synnaxlabs/foundation/issues/2019#issuecomment-6268955917), on the
  plan https://github.com/synnaxlabs/foundation/issues/2019#issuecomment-6268783749.
