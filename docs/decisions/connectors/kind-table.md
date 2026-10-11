- **KIND TABLE** `kind::Kind` is typed: an associated `Config` and `impl Future`
  methods. `kind::Table` erases it inside `connector` with a private trait that takes
  the `Document` and parses again, so callers see one concrete type with no `Any` and
  no downcast. An unknown kind is the diagnostic `connector.unknown-kind`, since the
  name comes from a file. A run or a discovery fails with one of three classes:
  `Config`, `Device`, and `Retry` (restart with backoff). Decided by the `connector`
  builder in the plan on #338, after `/eb-review`; approved by the coordinator
  (#338). After `Config`, the connector stops until the next spec change, also one
  that changes only another definition, since the fix can be there: the channels that
  a reader selects, or the settings of another connector on its endpoint
  (`laptop.architect-2`, 2026-10-11T03:35:05Z:
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6105068890).
  Supersedes "After `Config`, the connector stops until a spec change changes it"
  (`laptop.architect-2`, 2026-10-09T05:10:43Z:
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6074718616).
  Supersedes "The supervisor stops; a spec change starts it again" of the plan on #338
  (https://github.com/synnaxlabs/foundation/issues/338#issuecomment-5994872059).
  The class of a hub error that `kind::Context::reader` or `writer` gives: `Config`,
  with one diagnostic with no span and the hub error's text, for an error that only a
  spec change fixes: reader `Empty` (`connector.no-channel`) and `ManyIndexes`
  (`connector.many-indexes`); writer `Unknown` (`connector.unknown-channel`), `Empty`
  (`connector.no-channel`), and `Home` with a lease that is not longer than zero
  (`connector.lease`). `Retry`, with the hub error: `Remote` and `Mesh` of each, since
  the home can move back, and a mesh that stops ends the run at the cancel after at
  most one backoff. `Device` is for the device or the endpoint, never the hub. An error
  that the open never gives panics with `invariant:`: writer `Home` `Unsynced`,
  because `Hub::writer` waits for mesh time; reader `Unsynced`, because no run starts
  before the node has mesh time, and mesh time stays once known; and reader
  `Transport`, `Refused`, `Message`, and `Pool`, because a named reader opens only at
  its home (`laptop.architect-2`, 2026-10-10T22:38:41Z:
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6102937379). The
  writer `Home` list and the codes: proposed in the plan on #1734
  (https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6104945733),
  approved by `laptop.architect-2` (2026-10-11T03:21:41Z,
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6104976908). The
  reasons: `laptop.architect` (2026-10-11T03:41:31Z,
  https://github.com/synnaxlabs/foundation/pull/2348#issuecomment-6105112575).
  `Table::check` takes where the file names the kind and puts `connector.unknown-kind`
  there; `discover` and `run` take their kind from the spec, which has no spans
  (`laptop.architect-2`,
  https://github.com/synnaxlabs/foundation/issues/1153#issuecomment-6051297152,
  2026-10-08 03:02 UTC). `Table::check` also puts there each diagnostic of the kind
  with no span, since a `Document` has none to place a missing attribute
  (`laptop.architect-2`,
  https://github.com/synnaxlabs/foundation/pull/1782#issuecomment-6051900967,
  2026-10-08 03:59 UTC).
  `Channels::counts` names the count channels of the kind,
  `<connector>.status.<count>`. `connector::status` names the status index (`time`)
  and the supervisor's channels (`state`, `class`, `restarts`, `backoff`, `error`)
  once, for the supervisor and for `config`. `Table::check` panics when a kind names a
  count that `connector::status` names in any case, a count of more than one segment,
  or one count twice in any case, since the kind's code is wrong (`laptop.architect-2`,
  2026-10-08T03:05:58Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6051331538,
  change 4 and answer 2; `laptop.architect-2`, 2026-10-09T13:37:08Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6082035824;
  `laptop.architect-2`, 2026-10-09T13:58:16Z:
  https://github.com/synnaxlabs/foundation/pull/2150#issuecomment-6082402451; for
  `backoff` and `error`, `laptop.architect-2`, 2026-10-08T06:29:21Z:
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186).
  `node` builds the table once, in `kinds`, and gives `ops::Node` an
  `Arc<kind::Table>`, the type that `supervisor::Config` takes, so that each kind is
  one value for the life of the process (`laptop.architect-2`, 2026-10-10T15:29:33Z:
  https://github.com/synnaxlabs/foundation/pull/2252#issuecomment-6099106464).
  The supervisors get the same `Arc`, from the one wiring site (`laptop.director`,
  2026-10-09T05:05:48Z:
  https://github.com/synnaxlabs/foundation/issues/1156#issuecomment-6074666909).
