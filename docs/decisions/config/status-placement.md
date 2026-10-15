- **STATUS PLACEMENT (#1821, 2026-10-10)** The status channels of a connector are placed
  with it (X24, X33). `config::plan::plan` places the index `<c>.status.time` with the
  placement of the connector `c`: its winner, home, standby, and copies. The data
  channels of the status follow their index, as each data channel does. Lost: an
  ordinary index, which makes each file that places a connector also select
  `<c>.status.time`, and a pattern of `spec::placement` that selects the status names
  with the connector, a special case for one kind of name in a crate that knows no
  connectors. Only `config` places indexes, so no other crate changes. Placement
  selection does not consider a status channel, so a pattern such as `plant.*` that
  matches a status name changes nothing. Decided by `laptop.architect-2`
  (2026-10-10T15:41:24Z,
  https://github.com/synnaxlabs/foundation/issues/1821#issuecomment-6099218399). A
  placement whose `select` matches only status names places nothing, as any placement
  that selects no channel, with no diagnostic (`laptop.architect-2`,
  2026-10-10T16:07:40Z,
  https://github.com/synnaxlabs/foundation/issues/1821#issuecomment-6099496528, which
  supersedes the `config.empty-placement` clause of
  https://github.com/synnaxlabs/foundation/issues/1821#issuecomment-6099218399). Only
  the connector writes its status: `config.implied-channel` refuses each connector whose
  kind writes a status channel (#2295; `laptop.architect-2`, 2026-10-10T22:35:37Z,
  https://github.com/synnaxlabs/foundation/pull/2257#issuecomment-6102913855).
  So the index can have no other home, and a rule that lets a placement give it one only
  makes an error that no file can fix (`laptop.architect-2`, 2026-10-10T15:41:24Z,
  https://github.com/synnaxlabs/foundation/issues/1821#issuecomment-6099218399). The
  connector is a writer of each index that it implies (PLAN SURFACE). Superseded: no
  rule counts the connector as a writer of an index that it implies (PLAN SURFACE;
  `laptop.architect-2`, 2026-10-13T01:21:52Z:
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6136011417).
  `config::plan::check` refuses definitions that lack a status channel of a connector or
  hold it as another channel, with `config.implied-channel` at no span, since a plan
  file that `plan` did not make can hold such a change, and the connector then cannot
  open its status. The check, and before it that of a kind that writes a status channel,
  are a stage after that of the kind table and the names, since each needs the table to
  take each connector, and no check of that stage needs another to pass (PLAN FILE).
  Lost: a new code, since the rule is that of `config.implied-channel`; and
  `Plan::definitions` filling in each status channel, since the applied spec then
  differs from the plan file that the caller checked. Decided by `laptop.architect-2`
  (#2256, 2026-10-12T21:39:19Z,
  https://github.com/synnaxlabs/foundation/pull/2498#issuecomment-6133099350; the reason
  of the stage, 2026-10-12T21:48:03Z,
  https://github.com/synnaxlabs/foundation/pull/2498#issuecomment-6133221337; the stage
  of the write check, 2026-10-12T22:33:22Z,
  https://github.com/synnaxlabs/foundation/pull/2498#issuecomment-6133885580).
  In the same stage, `check` refuses each channel that no connector implies and that
  has an edge to a status channel of a connector, with `config.implied-channel`, once
  for each status channel that it points at. Only the connector writes its status, and
  a second writer on the status index can hold its control gate
  (`laptop.architect-2`, 2026-10-13T01:13:16Z:
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6135917943).
  `plan` refuses such an edge in files with `config.unknown-channel`, and #2513 gives
  it `config.implied-channel`. Lost: a new code, since the rule is that of
  `config.implied-channel`; and the rule in the stage of the rules of `plan`, which
  count the writers of an index, a count that a second writer already makes false.
