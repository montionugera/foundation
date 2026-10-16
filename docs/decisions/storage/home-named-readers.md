- **HOME NAMED READERS (#1742, #1851)** `Shard::open_named_complete` and
  `open_named_latest` open a reader by its `reader::named::Key`, and `Shard::ack` moves
  the position of a named complete reader. Before the node first has mesh time, a named
  open gives `reader::Unsynced`, because a hold ends at a mesh time stamp. A named
  complete reader opens at its last ack within its hold, else at the live tail, and
  ends `Behind` when its position is below the frames that memory keeps. Until #274
  reads from disk, it stays `Behind` at each open within its hold. Seqs lost for no
  room after its position are no reason for `Behind`: its next frame gives them as its
  gap (#2553, `laptop.architect`, 2026-10-15T18:12:25Z:
  https://github.com/synnaxlabs/foundation/issues/2553#issuecomment-6199035922). Its
  close starts its hold. A hold that ended goes at the next named complete open of its
  index, until #274 ends it at `Readers::deadline`. An open of the same key takes the
  old session over, and `reader::Opened::replaced` names it. `home::reader` re-exports
  `delivery::{Error, Position, named}`, so `hub` does not depend on `delivery`
  (`laptop.architect`, 2026-10-08T11:12:45Z:
  https://github.com/synnaxlabs/foundation/pull/1863#issuecomment-6058607367).
  `home` drops the position records of `delivery` until #274 appends them to the index
  log, so a reopen after a restart starts at the live tail. Lost: one open that takes a
  `delivery::Reader`, because only a named open can fail. Decided by `laptop.architect`
  (2026-10-08T10:01:19Z:
  https://github.com/synnaxlabs/foundation/issues/1742#issuecomment-6057419592). The
  subject in the key and the order of the PRs: `laptop.architect`
  (2026-10-08T10:15:04Z:
  https://github.com/synnaxlabs/foundation/issues/1851#issuecomment-6057659053).
  Amended (#274 B1): a named complete reader that opens within its hold reads the
  frames after its position from disk, then the live ones. It ends `Behind` only when
  it reaches a seq that the buffer lost for no room, also at each later open within its
  hold. The position records and `Readers::deadline` stay as above until later parts
  of #274 (`laptop.architect`, 2026-10-15T16:29:36Z:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6197039098).
  Amended (#2725, with #2723 and #2553): no case gives `Behind` now. A complete session
  that misses a frame reads it from disk, and a seq lost for no room comes as the gap of
  the next frame. The variants stay until #2790 removes them.
