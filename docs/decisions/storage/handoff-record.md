- **HANDOFF RECORD (#191)** The home records each handoff that `Gate::handoff` gives
  (GATE RULES) as a buffer entry on the live path of the index, with tag `HANDOFF`,
  `len` 0, and `first` at the live tail. It records a handoff after the gate input that
  gave it and before the next stored frame of its index. It records it before the next
  input too, except for a handoff with no room or after a failed commit
  (`laptop.architect`, #2318, 2026-10-11T10:46:58Z:
  https://github.com/synnaxlabs/foundation/pull/2318#issuecomment-6108217840).
  Supersedes "before the next input or frame" of
  https://github.com/synnaxlabs/foundation/pull/402. Its bytes are empty when no
  writer holds control, else `[authority: u8]` then the holder's subject as UTF-8; the
  entry length gives the subject's length. A restart or a failover starts the gate from
  the last record (X18): `Gate::recover` with its holder, or `Gate::new` when it names
  none. `Shard::open` restores each gate, with `GRACE` (10 s) from the open, and the
  gate moves in at the first carry of its slot, also after the grace
  (`laptop.architect`, #275, 2026-10-10T04:04:50Z:
  https://github.com/synnaxlabs/foundation/issues/275#issuecomment-6093580361). The
  experiment that sets the gate grace (`docs/decisions/open/parameters.md`), or a test
  that needs another value, makes `GRACE` a `Config` field (`laptop.architect`, #275,
  2026-10-10T04:04:50Z, item 4). A lease or a grace that ends with no call records its
  handoff at its end: the hub's gate task calls `Shard::advance` at `Shard::deadline`,
  on the monotonic clock of the hub's `clock::Reader`. `Shard::advance` records the
  handoff of each index that came due, with or without an open writer: `Shard::carry`
  takes the channel key of the index with its slot, and the hub wakes its gate task at
  each carry, as a carry can make `Shard::deadline` earlier (`laptop.architect`, #2314,
  2026-10-10T22:57:44Z, 2026-10-10T23:13:18Z, 2026-10-10T23:22:58Z, and
  2026-10-11T01:00:41Z:
  https://github.com/synnaxlabs/foundation/issues/2314#issuecomment-6103084268,
  https://github.com/synnaxlabs/foundation/issues/2314#issuecomment-6103202889,
  https://github.com/synnaxlabs/foundation/issues/2314#issuecomment-6103276258, and
  https://github.com/synnaxlabs/foundation/issues/2314#issuecomment-6103996718).
  Before the node first has mesh time, such a handoff waits as one with no room does.
  Lost: the channel key in `buffer::Stored`, which each read on the data path makes,
  for one control case. A handoff is not on disk while the ring or the pool has no
  room for it, and a restart or a failover in that time starts the gate from the last
  record on disk, as above. Only room held back for one handoff of each index, in the
  pool and in the ring, closes this. Trigger to look again: a measured case where a
  ring or a pool stays full across a handoff in a workload on the milestone path. Until
  #2385, such a handoff waits for
  the next write that is not a resend, writer open or close, or end of a lease on its
  index (INDEX FRAMES), also after the room comes back (`laptop.architect`, #2385,
  2026-10-11T08:16:48Z, the OK of the deferral, item 4, and the trigger of its "Lost"
  list: https://github.com/synnaxlabs/foundation/issues/2385#issuecomment-6107045587;
  the words for a restart in that time and the list of calls: `laptop.architect`, #2318,
  2026-10-11T10:46:58Z,
  https://github.com/synnaxlabs/foundation/pull/2318#issuecomment-6108217840). A shed
  keeps it for the next carry of the index (HOME SURFACE).
  Trimming must keep the last record of each index (#406). Until it does, a trim
  (STORE TRIM) can free that record, and a holder whose record a trim freed gets no
  grace after a restart. Retention deletes nothing (decided by `laptop.architect`,
  2026-10-07T12:30:53Z and 2026-10-07T12:59:37Z:
  https://github.com/synnaxlabs/foundation/issues/1377#issuecomment-6037946637 and
  https://github.com/synnaxlabs/foundation/issues/1377#issuecomment-6038431739).
  Supersedes https://github.com/synnaxlabs/foundation/pull/402 in its clause that
  retention can remove that record. The layout is part of the disk format version (C9d).
  Copy mode checks each record once where remote records enter (X43), and the read after
  it panics on a bad record. Decided by the `write-path` builder; approved by the
  coordinator (#191).
