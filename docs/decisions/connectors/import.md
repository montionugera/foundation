- **IMPORT (C1, #2705)** `import` writes the config of chosen points into the file of
  the connector block and only adds. `connector` gives the two parts that do not need
  the files. `Kind::block(tag, facts, channel) -> Result<Vec<Block>, Vec<Diagnostic>>`
  gives the blocks to add to the body of the connector block, so that it reads or writes
  the point `tag` as the channel `channel`. It gives diagnostics, as `parse` and `check`
  do. Its blocks hold only how the kind reaches the point, such as the tag and the
  access: `config` writes the sample type and the unit in the `channel` block, so no
  fact is written or filled twice. A point whose access is `Write` gets a write block.
  Each other point, with no access too, gets a read block, since a write to a device
  needs the user's choice. It takes the tag and the facts of a `Shape::Point`, not an
  `Entry`, so a kind never gets a `Folder` or an `Unreadable`, and no kind needs the
  path. For each value that the kind needs and the browse does not give, the blocks hold
  `import::fill(what)`, the value `fill("<what>")`, which `plan` refuses. `fill` is in
  `connector::import`, not in `document`: it means "`plan` refuses it", a rule of import
  and plan, not of the syntax-neutral document. Its read goes beside it, and `config`
  calls the read. The default of `Kind::block` gives `connector.no-import`, as
  `no-browse` and `no-discover` do: a default that cannot fail adds a channel that
  nothing reads, and one that panics stops the node on the defect of a kind.
  `Table::block` finds the kind by name, as `Table::discover` does, and gives
  `connector.unknown-kind` for a name that it does not have. `Kind::tag(block) ->
  Option<String>` gives the tag of the point that a block of the body of a connector
  block reads or writes, in the form that a browse of the kind gives it, or `None` when
  it names no point. The default gives `None`, and `Table::tag` finds the kind as
  `Table::block` does. So an import finds a pick that is present by the kind's compare
  of node IDs, not by text: the OPC UA kind gives `i=2258` for a written `ns=0;i=2258`
  (`laptop.architect-2`, 2026-10-18T02:10:56Z, item 2:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6244589074). Each
  kind that gives blocks has a property test that `tag` of each block that `block(t,
  ..)` gives is `Some(t)` (`laptop.architect-2`, 2026-10-18T02:44:21Z, finding 4:
  https://github.com/synnaxlabs/foundation/pull/3102#issuecomment-6244852270).
  `import::names(connector, start, picks, taken)` gives the channel name of each pick,
  in pick order: each given name as it is, then the default `<connector>.<path>` of each
  other pick. Each name of the path below `start` is one segment, with `_` in place of
  each character other than an ASCII letter, a digit, `_`, and `-`. A default that is in
  `taken`, is a given name, or is an earlier default, also in another case, gets the
  first free suffix of `_2`, `_3`, and on. A default longer than `Name::MAX_BYTES`, with
  its suffix, is `None`, and the summary gives it as not added, with the fix "Give it a
  name". `taken` holds each name that the files define or imply, such as the status
  channels of a connector. The path of a pick with no given name that is not below
  `start` panics: the walk gives each path below its start, so it is a defect of the
  caller. The fix of a given name that `add` refuses, "Give another name, or leave it
  out for `<default>`", needs no other item: `config::import::add` calls `names` again
  with the refused names as `None`, on the failure path only. A given name that does not
  parse is refused by `serve` before the walk (IMPORT OP), with `ops.bad-name`: each
  such name at once, with its path, its text, and the cause, and the fix "Give another
  name, or leave it out for the default name". Lost: an item of `config::import` that
  gives the defaults before the walk, so that this fix names the default, which adds a
  public item for a rare error (`laptop.architect-2`, 2026-10-18T06:08:27Z:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6246405490, which
  supersedes, for a name that does not parse only, the `<default>` in the fix of the
  ruling on decision 5). No name of a path is empty: `walk` refuses an entry that a kind
  gives with an empty name (BROWSE), and `Path::from_str` refuses an empty name. `names`
  takes the characters of a segment from `Name::segment_char` (A3), so its `expect`
  checks an invariant that holds. Lost: a namer with state (`give`, then `next`), where
  nothing makes the caller give each given name before the first default; a `Document`
  from `Kind::block`, where an attribute of one pick merges into the one connector body
  and collides on a second pick; and the run error `Error`, which has variants that a
  hook with no I/O cannot give. The person's words of decision 5, through
  `laptop.monitor`, 2026-10-15 17:28 UTC: "Yeah, that's fine"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198090285); given
  names first, and one set of name rules in `config`: `laptop.architect-2`, on that
  decision. `Kind::block` by tag and facts, the default `connector.no-import`,
  `Table::block`, `fill` in `connector::import`, `names` with `start`, and the fix text
  by a second call: `laptop.architect-2`, 2026-10-16 10:06 UTC, on the plan of
  `box1.builder-3`
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6213124219).
  Supersedes
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198270136,
  parameter 2: the hook `Kind::block(&self, entry, channel) -> Document`. Blocks in
  place of a `Document`, diagnostics in place of `Error`, the facts that `config`
  writes, the block for each access, and #2868: `laptop.architect-2`, 2026-10-16 14:31
  UTC, on the round 1 architecture review of #2859
  (https://github.com/synnaxlabs/foundation/pull/2859#issuecomment-6217747802). `fill`
  is a reserved call. `config::check` reads each value at any depth, also in the config
  of a kind, and each call `fill` gives only a problem of `fill` at its span, after
  `config.private-key`, with the fix "Write the value in place of `fill`".
  `fill("<what>")` gives `config.unfilled`, "the import found no value for <what>". A
  call `fill` of other arguments gives `config.bad-fill`, "`fill` takes one string". A
  call `fill` inside a call `fill` gives no problem of its own. `config::plan::check`
  gives the same problems for each connector config of a plan file, after
  `config.private-key`. `import::unfilled` gives the call `fill` that a value is, as an
  `import::Fill`: `Of(<what>)` or `Bad`. So only `connector` knows the name and shape of
  the call. No kind may give a call `fill` a meaning of its own.
  `config::import::{channel, index}` writes the `channel` blocks of an import, so the
  grammar of a `channel` block stays in the crate that parses it, and no kind or `ops`
  code writes one. A missing sample type is `fill("the sample type")`. The unit is what
  `spec::unit::Unit::given` gives (A16). An empty text or code counts as none. When
  `Unit::given` gives none for a text or a code, the unit is `fill("the unit, which the
  device gives as <what>")`, so nothing drops in silence. `<what>` is `"<unit>"`, `the
  code "<code>"`, or `"<unit>" with the code "<code>"`. A text that reads is the unit,
  and a code that does not read drops. A sample type that holds no number gets no unit.
  Lost: a value kind `document::Kind::Fill`, where each front end writes and reads it
  and a rule of import goes into the syntax-neutral model; and a refusal of `fill` in
  the parse of each kind, one rule in each kind, where a `channel` block is no kind's.
  `unfilled`, `config.unfilled`, `config::import`, the reserved call, and the fix text:
  `laptop.architect-2`, 2026-10-16 14:58 UTC
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6218269338). The
  unit map: `laptop.monitor` objected to a `fill` for each unit that is not ASCII ("an
  import of 500 points in `°C` stops 500 times"), and `laptop.architect-2` ruled the
  order of `Unit::given`, 2026-10-16 15:00 UTC
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6218302597).
  `Unit::given`, `Facts::unit_code`, and a public `DataType::numeric`:
  `laptop.architect-2`, 2026-10-16 15:04 UTC
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6218371040).
  `import::FILL`, `config.bad-fill`, and the `fill` of a code that does not read:
  `laptop.architect-2`, 2026-10-16 15:55 UTC, on the round 1 architecture review of
  #2883 (https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219336388).
  `import::Fill` in place of a public `import::FILL`, and no problem for a `fill` inside
  a `fill`: `laptop.architect-2`, 2026-10-16 16:06 UTC, on the round 2 review of #2883
  (https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219539273).
  Supersedes https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219336388,
  item 3: a public `import::FILL`. `config::plan::check` refuses a `fill` in a connector
  config: `laptop.architect-2`, 2026-10-16 16:27 UTC, on the round 3 fix of #2883
  (https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219969435).
  `config::import::add(documents, connector, start, picks, kinds)` gives the document of
  the file of the connector block with the blocks of each `Pick` added, and an `Outcome`
  for each pick: `Added { name, fills }`, `Unnamed`, or `Present`. A pick is present
  when a block of the connector body or an earlier pick that adds a channel has its tag
  as `Table::tag` gives it, and takes no name (`laptop.architect-2`,
  2026-10-18T03:34:05Z, ruling A on round 1 finding 4 of #3109:
  https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245234973, on item 3
  of https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245080635). A pick
  that ends `Unnamed` makes no later pick of its tag present, so `add` names until no
  pick changes. Lost: one pass that names each pick that the body does not have, then
  marks the later picks of its tag `Present`, as it spends a suffix on, and checks the
  given name of, a pick that ends `Present`. The other picks take names in this order:
  each given name, then the index, then the defaults of `import::names`. The index is
  that of the first channel that the connector writes whose block names one, else the
  new `<connector>.time`, or the first free of `_2`, `_3`, and on, with case folded,
  added only when a pick adds a channel. The blocks of `Table::block` go after the last
  item of the connector body; the new index and the `channel` blocks go at the end of
  the file. A given name that is reserved, that the files define or imply or that a
  connector writes, in any ASCII case, or that two picks get, fails the call before the
  write, with each diagnostic at once: `config.reserved-name`, or
  `config.duplicate-name` with the path and the name, or one `config.duplicate-name`
  that names both paths. Each has the fix "Give another name, or leave it out for
  `<default>`", with the default of the run with each refused name left out, or "Give
  another name" when that run gives none, refuses a name, or has a pick that is not
  present, with no name, whose path is not under `start`. `add` gives no problem of a
  block outside the connector block other than `config.private-key`. An `index` that is
  not a name, in the `channel` block of the first channel that the connector writes that
  has one, gives its diagnostic. The implied names of the other connectors with their
  counts, the diagnostic of a bad `index`, "or that the connector writes", and "other
  than `config.private-key`": `laptop.architect-2`, 2026-10-18T03:14:04Z, items 2 and 4
  (https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245080635), and
  2026-10-18T03:34:05Z
  (https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245234973). "or that
  a connector writes", with the channels that the other connectors write, the fix
  default of the run with each refused name left out, the panic of a bad `fill`, and the
  one-pass shape that lost: `laptop.architect-2`, 2026-10-18T04:22:50Z
  (https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245610148); "or
  refuses a name": `laptop.architect-2`, 2026-10-18T04:32:21Z
  (https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245680650); "or has
  a pick that is not present, with no name, whose path is not under `start`", and the
  panic without "or a refused one": `laptop.architect-2`, 2026-10-18T04:54:49Z
  (https://github.com/synnaxlabs/foundation/pull/3109#issuecomment-6245847948).
  `Outcome`, `Pick`, `add`, the errors, and the name order: `laptop.architect-2`,
  2026-10-18T02:10:56Z, items 2, 3, 4, 5, and 11
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6244589074).
  Supersedes
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6213124219, item 5:
  the second call of `names` by `ops`.
- **IMPORT OP (C1, #2705)** `foundation import <CONNECTOR> <FILES>... (--point <PATH>...
  | --match <PATTERN>) [--start <PATH>] [--name <NAME>=<PATH>]...` and the MCP tool
  `import` (`connector`, `files`, `points` of `{path, name}` with `name` optional, or
  `match`, and `start`) add the config of points to the file of the connector block.
  `--name` also picks its point, after each `--point`. The node makes the change:
  `serve` runs the checks of `browse` before the device, then, in order, `ops.no-pick`
  for no pick, an argument error for both forms, `ops.import-limit` for more than 500
  paths, `ops.bad-point` with each path that does not parse, `ops.duplicate-pick` with
  each path picked twice, `ops.bad-name` with each given name that does not parse, and
  `ops.bad-pattern`. Each of the three holds a list, with one problem for each, so one
  run reports each bad text, and none does device I/O. Then it walks with
  `Scope::Points` or `Scope::Match`. A walk that ends `Budget` gives `ops.import-budget`
  and one that ends `Limit` gives `ops.import-limit`, so no import adds part of a match
  in silence. The fix of `ops.import-limit` is "Narrow the match, or pick at
  most 500 points", and of `ops.import-budget` "Narrow the start or the match, or pick
  fewer points", since a walk of points and a match both give each.
  A picked point that the kind cannot read or write gives
  `ops.unreadable-point`, one diagnostic for each, with its reason; a match leaves each
  out and counts it. Then `config::import::add`, then the new text by `FrontEnd::update`
  of the file. The reply is the JSON line of the output, then the new text, and a reply
  over the bound of a body gives `ops.large-import`. The CLI and the MCP server write
  the new text through `Scope`, so the limits of `foundation mcp` hold. They write
  nothing when it is the text that they sent, and when the file no longer holds the sent
  text they give `ops.file-changed` and write nothing. On `os`, an import that writes
  a regular file makes it anew, with mode 0600 and nothing of the old file, so a file
  that the user opened to a group or to other users is closed to them again after each
  import. A symbolic link, a device, or a pipe at the path takes the new text in
  place, with its mode kept, and a crash can leave part of it (ENV SEAMS). Lost: the
  permission bits of the old file kept on the new one, which open the
  file to a user that the old file did not when its group changes or when an access
  control list applies. `--json` and the MCP result are `{file, channels: [{name, path,
  type}], index, fills: [{name, what}], unnamed: [{path, fix}], present, unreadable}`,
  with no added blocks: `index` is the added index or `null`, `type` is `null` where the
  import wrote a `fill`, and `fills` holds each channel with a `fill` in any of its
  blocks. The text output is one line for the file and the count, one for each channel
  as `<name> <path> <type>`, then one line for the index, each fill, each unnamed point,
  and each count that is not 0. `TABLE` holds `import` with `read_only: false` and
  `destructive: false`. Each code has one fix. The `instructions` of the MCP
  `initialize` result is the agent guide: `browse`, `import`, a value for each `fill`,
  `plan` shown to the person, and `apply` only when the person agrees, and `status`. It
  leaves out `discover`, which is not in C1, so no step names a tool that the server
  does not have. Deferred: picks by tag. Trigger: a user who has a list of tags and no
  paths. Deferred: the added blocks in the JSON (B of decision 4). Trigger: the person
  asks. The person's words of decision 4, through `laptop.monitor`, 2026-10-15 17:25
  UTC: "I think A is fine for now"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198070930); of
  decision 6, 2026-10-15 17:33 UTC: "Ok this is good"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198173531); of
  decision 8, 2026-10-15 17:39 UTC: "A is fine"
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198270136). The
  order of the checks, the budget, the duplicate pick, the write check, `TABLE`, the
  codes, and the trigger of picks by tag: `laptop.architect-2`, 2026-10-18T02:10:56Z,
  items 6 to 11
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6244589074). The
  JSON: `laptop.architect-2`, 2026-10-18T02:36:08Z
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6244788485), which
  supersedes the words "with the added documents" of item 4 of
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198070930. The
  parse of each point and given name in `serve`, each in a list: `laptop.architect-2`,
  2026-10-18T06:19:04Z
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6246489783). The
  mode 0600: `laptop.architect-2`, 2026-10-18T06:03:37Z
  (https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6246366941), which
  supersedes the kept bits of item 8 of
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6244589074. The
  guide without `discover`: parameter 5 of
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6198270136.
