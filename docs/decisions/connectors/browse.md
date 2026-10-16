- **BROWSE (C1, #2705)** Three operations: `discover` finds devices, `browse` looks
  inside one and changes nothing, and `import` writes the config of chosen tags into the
  file and only adds. One engine serves every front end: the CLI with `--json` and the
  MCP tools, both from the operation table, each designed first for an agent. No TUI in
  C1; a front end holds no device state and no field of a kind, so a TUI later needs no
  engine change. Every kind gives one tree shape, `connector::browse`: an `Entry` has a
  `Path`, a `tag` (the address in the device; two entries with the same tag are one
  entry of the device), and a `Shape`: `Folder`, `Point(Facts)`, or `Unreadable(reason)`
  for a point that the kind cannot read, such as an array. `Facts` holds the sample
  type, the unit, and read or write, each `None` when the source cannot know it;
  `import` writes such a fact as `fill("<what>")`, which `plan` refuses. `browse` has
  two forms: one level, paged by a `Cursor` of the kind, or each entry that is not a
  folder whose path from the start matches a `Pattern` (`/` splits names, `**` alone
  matches any number of names, `*` any text in one name, and `\` takes the next
  character as it is). It is not a `types::name` selector, where `*` matches one whole
  segment of a dot-separated name: a device's names hold any character, a glob looks
  inside one name, and the walk asks at each folder whether a longer path can match.
  Values are read only when the `Query` asks, in one call for the points that it gives.
  A new filter is a new field of `Query`. The walk lives once in `connector`
  (`browse::walk`). It finds the start from the root, one name at a time; a name that is
  not there or not a folder gives `connector.no-folder`. A pattern walks depth first in
  device order, browses no folder that no match can be under, gives at most `LIMIT`
  (500) points and ends `Limit` when one more matches, and looks at most at `VISITS` (10
  000) entries, also the entries that find the start, where a page with no entries
  counts as one (a server may give a continuation point with no references), then ends
  `Budget`. With `Limit`, `import` says "More than `<limit>` points match, the most for
  one import; narrow the pattern, or import in parts", not a count. It browses no folder
  whose tag and pattern state are those of a folder that it holds open, so a loop in a
  graph such as an OPC UA address space ends, and a path that a pattern names through a
  loop still matches. A pattern ends `Budget` when it holds `DEPTH` (64) folders open,
  its start with them, and a match can be under one more, so a chain of folders that a
  server makes cannot hold the walk's memory. A kind gives each entry as a `Child`, with
  its name and not its path, and the walk makes the path of each entry from its
  folder's, so an entry holds one link of its own at any depth of the start, and the
  walk holds at most `DEPTH` pages that the device sent. No path that the walk gives has
  an empty name, and the record of each kind says what it gives for an empty name of its
  device, so the text of each path parses. When a kind breaks one of these rules of
  `Device`: a page over `LIMIT`, an empty name, a count of values that is not the count
  of tags, `walk` gives `Error::Config` with `connector.broken-browse`, whose message
  names the rule, then what broke it, and the browse fails with no retry. It never
  panics: in release a panic stops the node and each of its connectors. The browse
  operation adds the connector and its kind to each error that it gives from `walk`. A
  kind implements `browse::Device` on its open session, so one walk is one session and
  can use a continuation point; a `Cursor` that a front end gets holds no session state.
  `level` takes a folder as the `Entry` that the device gave, never a path, and gives at
  most `LIMIT` entries a page; `values` gives `types::sample::Value`, so there is one
  value model. Trigger for a text value: a kind that reads a `string` channel. Lost: a
  count of matches up to the budget; trigger: the agent test (PR 9) shows a task with
  two over-limit refusals one after the other.
  `import --match` uses the same walk, so the points that `browse --match` shows are the
  points that `import --match` takes.
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
  shape `Unreadable`, the limit, and the PR list: `laptop.architect-2`, 2026-10-15 17:32
  UTC (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198270136).
  `browse::Device`, `level` by entry, the page bound, `sample::Value`, the name
  `Malformed`, and "more than `<limit>`" in place of the count of decision 6:
  `laptop.architect-2`, 2026-10-16 04:00 UTC, on the plan of `box1.builder-3`
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207771367). One
  session for the whole browse: the person, through `laptop.monitor`, 2026-10-16
  03:54:58 UTC, "As long as thats the industry standard browsing practice I approve"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207717907). It is:
  OPC UA Part 4 requires BrowseNext on the session of the Browse that it continues. A
  page with no entries counts as one visit, not a panic: `laptop.architect-2`,
  2026-10-16 04:36 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208174481). `DEPTH`,
  the shared names, and the empty name: `laptop.architect-2`, 2026-10-16 04:42 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208244581). An
  error, not a panic, for a break of `Device`: the person, through `laptop.monitor`,
  2026-10-16 04:54:26 UTC, "Yes", on "Should a browse bug return an error instead of
  crashing the node?"
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208381924). The
  shape of the error: `laptop.architect-2`, 2026-10-16 04:56 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208402982); the
  private `Broken` and a `Path::child` that cannot fail: `laptop.architect-2`, 04:57 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208408668). The loop
  check by tag and pattern state, `DEPTH` as the count of open folders (it replaces the
  meaning of 04:42 UTC), and the four rules that `walk` checks: `laptop.architect-2`,
  05:23 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208716424). The
  `types::name` sentence: `laptop.architect`, 05:20 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208688678). A `Path`
  that shares the path of its folder (it replaces the bound of a deep start of 05:23
  UTC): `laptop.architect-2`, 05:30 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208798023). One tag,
  one entry: `laptop.architect-2`, 2026-10-16 05:04 UTC
  (https://github.com/synnaxlabs/foundation/pull/2823#issuecomment-6208496065).
  `VISITS`: the walk's own time, median of 100 on box1 in release, is 1.1 ms over a flat
  tree of 9 999 points and 0.9 ms over a chain of 63 folders of 150 points each; PR 5 of
  the PR list measures the cost of the OPC UA sim server. The two triggers come from the
  ruling of 2026-10-16 04:00 UTC. A page of `Child` entries, with a name and no path,
  and a private `Path::child`: `laptop.architect-2`, 2026-10-16 05:50 UTC
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6209047339).
