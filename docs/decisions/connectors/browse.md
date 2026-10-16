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
  The walk lives once in `connector` (`browse::walk`). It finds the start from the
  root, one name at a time; a name that is not there or not a folder gives
  `connector.no-folder`. A pattern walks depth first in device order, browses no folder
  that no match can be under, gives at most `LIMIT` (500) points and ends `Limit` when
  one more matches, and takes at most `VISITS` (10 000) entries from the device, also
  the entries that find the start, then ends `Budget`. With `Limit`, `import` says
  "More than `<limit>` points match, the most for one import; narrow the pattern, or
  import in parts", not a count. A kind implements `browse::Device` on its open
  session, so one walk is one session and can use a continuation point. `level` takes
  a folder as the `Entry` that the device gave, never a path, and gives at most `LIMIT`
  entries a page; `values` gives `types::sample::Value`, so there is one value model.
  `import --match` uses the same walk, so the points that `browse --match` shows are
  the points that `import --match` takes.
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
  `browse::Device`, `level` by entry, the page bound, `sample::Value`, the name
  `Malformed`, and "more than `<limit>`" in place of the count of decision 6:
  `laptop.architect-2`, 2026-10-16 04:00 UTC, on the plan of `box1.builder-3`
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207771367). One
  session for the whole browse: the person approved it, on the condition that it is
  industry practice, which OPC UA Part 4 makes a rule.
  `VISITS`: the walk's own time over a tree of 10 000 entries is 1.8 ms (median of
  100, box1, release); PR 5 of the PR list measures the cost of the OPC UA sim server.
