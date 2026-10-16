- **HOME NAMED READERS (#1742, #1851)** `Shard::open_named_complete` and
  `open_named_latest` open a reader by its `reader::named::Key`, and `Shard::ack` moves
  the position of a named complete reader. Before the node first has mesh time, a named
  open gives `reader::Unsynced`, because a hold ends at a mesh time stamp. A named
  complete reader opens at its last ack within its hold, else at the live tail, and
  ends `Behind` when its position is below the frames that memory keeps. Until #274
  reads from disk, it stays `Behind` at each open within its hold. Superseded by the
  amendment of #2725 below. Seqs lost for no room after its position are no reason for
  `Behind`: its next frame gives them as its gap (#2553, `laptop.architect`,
  2026-10-15T18:12:25Z:
  https://github.com/synnaxlabs/foundation/issues/2553#issuecomment-6199035922). Its
  close starts its hold. A hold that ended goes at `Readers::deadline` (Amended (#274
  A)). An open of the same key takes the old session over, and
  `reader::Opened::replaced` names it. `home::reader` re-exports
  `delivery::{Error, Position, named}`, so `hub` does not depend on `delivery`
  (`laptop.architect`, 2026-10-08T11:12:45Z:
  https://github.com/synnaxlabs/foundation/pull/1863#issuecomment-6058607367).
  Superseded by Amended (#274 A): `home` appends the position records to the index
  log, and a reopen after a restart resumes at the last recorded ack. Lost: one open
  that takes a
  `delivery::Reader`, because only a named open can fail. Decided by `laptop.architect`
  (2026-10-08T10:01:19Z:
  https://github.com/synnaxlabs/foundation/issues/1742#issuecomment-6057419592). The
  subject in the key and the order of the PRs: `laptop.architect`
  (2026-10-08T10:15:04Z:
  https://github.com/synnaxlabs/foundation/issues/1851#issuecomment-6057659053).
  Amended (#274 B1): a named complete reader that opens within its hold reads the
  frames after its position from disk, then the live ones. It ends `Behind` only when
  it reaches a seq that the buffer lost for no room, also at each later open within its
  hold. Superseded by the amendment of #2725 below. The position records and
  `Readers::deadline` come in Amended (#274 A) below (`laptop.architect`,
  2026-10-15T16:29:36Z:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6197039098).
  Amended (#2725, with #2723 and #2553): no case gives `Behind` now. A complete session
  that misses a frame reads it from disk, and a seq lost for no room comes as the gap of
  the next frame with a sample (`laptop.architect`, 2026-10-16T02:09:51Z:
  https://github.com/synnaxlabs/foundation/pull/2794#issuecomment-6206458946). The
  variants stay until #2790 removes them (`laptop.architect`, 2026-10-16T00:46:53Z:
  https://github.com/synnaxlabs/foundation/issues/2725#issuecomment-6205434126).
  Amended (#274 A): a home appends the record of each named complete reader of an index,
  open or held, as one entry of tag 2 on its live path, version 1. It appends the entry
  after a named open, a close, and a takeover, and at a tick when a record changed: an
  ack that moved a named position, or a hold that ended at the mesh midpoint. The tick
  comes each second while a named complete reader is open, or records changed or wait
  for mesh time; else when the next hold ends. `Shard::open` keeps the newest entry of
  each index, and a carry with mesh time restores it. A session open at the crash counts
  as closed at the restore, so a carry before mesh time waits for it. A shed appends an
  entry with no record. With no mesh time or no room for it, a restart restores the
  holds; a later carry in the same run does not. A named complete open that adds a
  record appends its entry before it opens. When the entry finds no block or no room,
  it opens nothing and gives `Unopened::Unrecorded`, and `hub` gives
  `reader::Error::Unrecorded`, so a restart never loses the frames of a first open.
  Another entry that finds no room or no block stays due for the next tick, so a
  restart before it resumes at an older position: a repeat, not a loss. Trimming must
  keep the newest named entry of each index (#406). Until it does, a trim (STORE TRIM)
  can free that entry, and a restart resumes the readers of its index at the live tail,
  as HANDOFF RECORD states for a handoff. One entry holds every record of
  its index, at most `min(Buffer::block_max, Layout::entry_max)` bytes: 546 bytes for a
  record of the longest names, so at least 7 records. A named complete open that adds a
  record past it gives `Unopened::Full`, and `hub` gives `reader::Error::Full`. An open
  of a reader that holds a record opens also when the records pass it, as on a smaller
  pool after a restart. A latest reader adds no record. #2833 holds records past one
  entry. Decided by `laptop.architect` (2026-10-16T05:22:26Z:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6208708096;
  `Unrecorded` and the trim gap until #406: 2026-10-16T07:08:03Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6210093371).
