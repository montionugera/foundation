- **BROWSE (C1, #2705)** Three operations: `discover` finds devices, `browse` looks
  inside one and changes nothing, and `import` writes the config of chosen tags into
  the file and only adds. One engine serves every front end: the CLI with `--json` and
  the MCP tools, both from the operation table, each designed first for an agent. No
  TUI in C1; a front end holds no device state and no field of a kind, so a TUI later
  needs no engine change.
  Every kind gives one tree shape, `connector::browse`: an `Entry` has a `Path`, a
  `tag` (the address in the device), and a `Shape`: `Folder`, `Point(Facts)`, or
  `Unreadable(reason)` for a point that the kind cannot read, such as an array.
  `Facts` holds the sample type, the unit, and read or write, each `None` when the
  source cannot know it; `import` writes such a fact as `fill("<what>")`, which `plan`
  refuses. `browse` has two forms: one level, paged by a `Cursor` of the kind, or each
  entry that is not a folder whose path from the start matches a `Pattern` (`/`
  splits names, `**` alone matches any number of names, `*` any text in one name, and
  `\` takes the next character as it is). Values are read only when the `Query` asks,
  in one call for the points that it gives. A new filter is a new field of `Query`.
  The walk of a pattern lives once in `connector` (`browse::walk`): depth first in
  device order, it browses no folder that no match can be under, gives at most
  `LIMIT` (500) points and ends `Limit` when one more matches, and takes at most
  `VISITS` (10 000) entries from the device, then ends `Budget`. A kind implements
  `browse::Device` (one level, and values) on its open session, so one walk connects
  once and can use a continuation point. `import --match` uses the same walk, so the
  points that `browse --match` shows are the points that `import --match` takes.
  The person's words, through `laptop.monitor`: decision 1, 2026-10-15 17:17 UTC, "Ok,
  this is fine. I approve"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6197989240);
  decision 2, 17:20 UTC, "Let's avoid the TUI picker for now and make a really good
  agentic approach instead. As logn as we can make a TUI later its all good"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198014500);
  decision 3, 17:23 UTC, "Yeah thats fine and this query achitecture can be evolved"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198047489);
  decision 6, 17:33 UTC, "Ok this is good"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198173531). The
  shape `Unreadable`, the limit, and the PR list: `laptop.architect-2`, 2026-10-15
  17:32 UTC (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198270136).
  `browse::Device` in place of a hook on `Kind` for each level: proposed by
  `box1.builder-3` in the plan of PR 1
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207706853).
  `VISITS`: the walk's own time over a tree of 10 000 entries is 1.8 ms (median of
  100, box1, release); PR 5 of the PR list measures the cost of the OPC UA sim server.
