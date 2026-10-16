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
  `connector.unknown-kind` for a name that it does not have. `import::names(connector,
  start, picks, taken)` gives the channel name of each pick, in pick order: each given
  name as it is, then the default `<connector>.<path>` of each other pick. Each name of
  the path below `start` is one segment, with `_` in place of each character other than
  an ASCII letter, a digit, `_`, and `-`. A default that is in `taken`, is a given name,
  or is an earlier default, also in another case, gets the first free suffix of `_2`,
  `_3`, and on. A default longer than `Name::MAX_BYTES`, with its suffix, is `None`, and
  the summary gives it as not added, with the fix "Give it a name". `taken` holds each
  name that the files define or imply, such as the status channels of a connector. The
  path of a pick with no given name that is not below `start` panics: the walk gives
  each path below its start, so it is a defect of the caller. The fix of a refused given
  name, "Give another name, or leave it out for `<default>`", needs no other item: `ops`
  calls `names` again with the refused names as `None`, on the failure path only.
  `names` keeps its own copy of the characters of a segment (A3), and its `expect` stops
  the import loudly when `types` allows fewer. The copy goes when `types` gives them
  (#2868). Lost: a namer with state (`give`, then `next`), where nothing makes the
  caller give each given name before the first default; a `Document` from `Kind::block`,
  where an attribute of one pick merges into the one connector body and collides on a
  second pick; and the run error `Error`, which has variants that a hook with no I/O
  cannot give. The person's words of decision 5, through `laptop.monitor`, 2026-10-15
  17:28 UTC: "Yeah, that's fine"
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
  (https://github.com/synnaxlabs/foundation/pull/2859#issuecomment-6217747802).
