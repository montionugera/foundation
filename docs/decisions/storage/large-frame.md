- **LARGE FRAME (#191)** The home refuses a write whose bodies no record of the ring
  holds, or with an entry whose parts, joined, pass `Buffer::block_max` of the shard's
  buffer, on either path, with `Large`. The home's header part counts in the entry's
  parts (#968). Decided, for the `Buffer::block_max` bound and the header part, by
  `laptop.architect` (2026-10-07T03:41:23Z):
  https://github.com/synnaxlabs/foundation/issues/1053#issuecomment-6030426386.
  Supersedes the bound of the largest block of the shard's pool,
  https://github.com/synnaxlabs/foundation/issues/968#issuecomment-6021211844. No seq
  moves and the home stores no part of the frame. The waiting handoffs of the frame's
  indexes are still recorded (HANDOFF RECORD). The writer splits the frame by samples or
  by indexes and writes each part. The home never splits a frame, because a frame
  applies whole (B7). Each handoff goes in its own append, so a handoff never makes a
  frame large. The size is checked only when the bodies are appended, after the
  handoffs: a frame whose handoff finds no room is lost (live) or refused with `Full`
  (backfill) before its size is known. Decided by the `write-path` builder (#191). After
  a failed commit, the home gives `Disk` before it checks the size, so a frame that no
  record of the ring holds, or with an entry whose parts, joined, pass
  `Buffer::block_max`, gets `Disk`, not `Large`, on either path (#1260). Decided by
  `laptop.architect` (2026-10-07T09:21:07Z):
  https://github.com/synnaxlabs/foundation/issues/1260#issuecomment-6034929252. The
  sentence departs from that ruling on the bound, approved by `laptop.architect`
  (2026-10-12T20:04:12Z):
  https://github.com/synnaxlabs/foundation/pull/2464#issuecomment-6131705729.
