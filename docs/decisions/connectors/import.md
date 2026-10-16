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
  calls `names` again with the refused names as `None`, on the failure path only. No
  name of a path is empty: `walk` refuses an entry that a kind gives with an empty
  name (BROWSE), and `Path::from_str` refuses an empty name. `names` takes the
  characters of a segment from `Name::segment_char` (A3), so its `expect` checks an
  invariant that holds. Lost: a namer with state (`give`, then `next`), where nothing
  makes the caller give each given name before the first default; a `Document` from
  `Kind::block`, where an attribute of one pick merges into the one connector body and
  collides on a second pick; and the run error `Error`, which has variants that a hook
  with no I/O cannot give. The person's words of decision 5,
  through `laptop.monitor`, 2026-10-15 17:28 UTC: "Yeah, that's fine"
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
  `fill` is a reserved call. `config::check` reads each value at any depth, also in the
  config of a kind, and each call `fill` gives only a problem of `fill` at its span,
  after `config.private-key`, with the fix "Write the value in place of `fill`".
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
  `import::Fill` in place of a public `import::FILL`, and no problem for a `fill`
  inside a `fill`: `laptop.architect-2`, 2026-10-16 16:06 UTC, on the round 2 review
  of #2883 (https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219539273).
  Supersedes https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219336388,
  item 3: a public `import::FILL`.
  `config::plan::check` refuses a `fill` in a connector config: `laptop.architect-2`,
  2026-10-16 16:27 UTC, on the round 3 fix of #2883
  (https://github.com/synnaxlabs/foundation/pull/2883#issuecomment-6219969435).
