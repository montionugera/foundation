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
  that takes a `delivery::Reader`, because only a named open can fail. Decided by
  `laptop.architect` (2026-10-08T10:01:19Z:
  https://github.com/synnaxlabs/foundation/issues/1742#issuecomment-6057419592). The
  subject in the key and the order of the PRs: `laptop.architect`
  (2026-10-08T10:15:04Z:
  https://github.com/synnaxlabs/foundation/issues/1851#issuecomment-6057659053).
  Amended (#274 B1): a named complete reader that opens within its hold reads the
  frames after its position from disk, then the live ones. It ends `Behind` only when
  it reaches a seq that the buffer lost for no room, also at each later open within its
  hold (`laptop.architect`, 2026-10-15T16:29:36Z:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6197039098).
  Superseded by the amendment of #2725 below.
  Amended (#2725, with #2723 and #2553): no case gives `Behind` now. A complete session
  that misses a frame reads it from disk, and a seq lost for no room comes as the gap of
  the next frame with a sample (`laptop.architect`, 2026-10-16T02:09:51Z:
  https://github.com/synnaxlabs/foundation/pull/2794#issuecomment-6206458946). The
  variants stay until #2790 removes them (`laptop.architect`, 2026-10-16T00:46:53Z:
  https://github.com/synnaxlabs/foundation/issues/2725#issuecomment-6205434126).
  Amended (#2790): the `Behind` variants are removed (HUB WIRE).
  Amended (#274 A): a home appends the record of each named complete reader of an index,
  open or held, in up to 64 parts, version 1. Part `p` is an entry of tag `2 + p` on
  its live path, of at most `Layout::ENTRY_MAX_MIN` (4032) bytes: 546 bytes for a record
  of the longest names, so at least 7 records a part and 448 an index. A record stays
  in one part while it exists, and a new one goes in the lowest part with room for it
  (first fit). A removed record keeps its key and its room in its part until the entry
  that drops it appends. An add of that key before then goes back in that part. A
  restart reads a prefix of the appends of the ring (S4), so a key is in one part only.
  An append writes only the parts whose records changed, in part order, and stops at the
  first part that finds no block or no room. A part that finds no block or no room keeps
  the state of its last entry, and a record that the append added at or past it loses
  its room. In memory, each index whose parts hold a record, carried or not, keeps the
  part and the last state of each key: at most about 257,728 bytes of names, plus about
  90 bytes for each key.
  It appends after a named open, a close, and a takeover, and at a tick when a record
  changed: an ack that moved a named position, or a hold that ended at the mesh
  midpoint. The tick comes each second while a named complete reader is open, or
  records changed or wait for mesh time; else when the next hold ends. `Shard::open`
  keeps the newest entry of each part of each index, and reads the tags from 2 up to
  the first tag that no index has an entry of. It panics on a key in two parts. A
  carry with mesh time restores the records. A session open at the crash counts as
  closed at the restore, so a carry before mesh time waits for it. A shed appends, in
  part order, an entry with no record for each part that holds one. With no mesh time,
  a restart restores the holds. When a part finds no block or no room, a restart
  restores the holds of that part and the parts after it; a later carry in the same run
  does not. A named complete open that adds a record, opens a held one, or takes over an
  open session with a new hold appends its parts before it opens, and its reader takes
  no frame until the commit that holds them. A takeover with the same hold of a session
  that waits for that commit waits for it too. When the part of its record, or a part
  before it, finds no block or no room, it opens nothing and gives
  `Unopened::Unrecorded`, and `hub` gives `reader::Error::Unrecorded`, so a restart
  never loses the frames of a first open, a reopen, or a takeover. After a failed
  commit, the open opens, and its reader ends with the error of the commit. Another
  entry that finds no room or no block stays due for the next tick, so a restart before
  it resumes at an older position: a repeat, not a loss. Trimming must keep the newest
  entry of each part of each index (#406). Until it does, a trim (STORE TRIM) can free
  such an entry, and a restart resumes the readers of its part at the live tail, and
  those of each later part when no index keeps an entry of its tag, as HANDOFF RECORD
  states for a handoff. A named complete open that adds a record that no part has room
  for gives `Unopened::Full`, with a `limit` of 257,728 bytes (64 parts of 4027 bytes of
  records), and `hub` gives `reader::Error::Full`. A reopen or a takeover never gives
  it, as a held record keeps its part. A latest reader adds no record. Parts: decided by
  `laptop.architect`
  (2026-10-17T00:37:51Z:
  https://github.com/synnaxlabs/foundation/issues/2833#issuecomment-6227434486), which
  supersedes one entry for each index, the open of a held reader over the limit, and
  the warning event of the rulings below. The room of a removed record goes free when
  the entry that drops it appends, not at its commit: decided by `laptop.architect`
  (2026-10-17T03:20:57Z:
  https://github.com/synnaxlabs/foundation/pull/2967#issuecomment-6228975494).
  Supersedes "a removal frees its room only at its commit" of item 1 of
  https://github.com/synnaxlabs/foundation/issues/2833#issuecomment-6227434486.
  Decided by `laptop.architect` (2026-10-16T05:22:26Z:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6208708096;
  `Unrecorded` and the trim gap until #406: 2026-10-16T07:08:03Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6210093371; the reopen
  in its hold and the failed commit: 2026-10-16T09:43:45Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6212725793; the read
  after the commit and the takeover: 2026-10-16T14:40:25Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6217913457; the
  refusal over the limit: 2026-10-16T14:41:12Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6217928514, reversed
  for a held reader: 2026-10-16T15:47:15Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6219185219; the
  warning event: `laptop.architect`, 2026-10-16T15:48:06Z:
  https://github.com/synnaxlabs/foundation/pull/2835#issuecomment-6219200565).
