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
  The class of a hub error that the supervisor's open of a reader or
  `kind::Context::writer` gives (the supervisor's open: `laptop.architect-2`,
  2026-10-11T05:23:35Z:
  https://github.com/synnaxlabs/foundation/issues/2354#issuecomment-6105802513, item 5):
  `Config`, with one diagnostic with no span and the hub error's text, for an error that
  only a spec change fixes: reader `Empty` (`connector.no-channel`) and `ManyIndexes`
  (`connector.many-indexes`); writer `Unknown` (`connector.unknown-channel`), `Empty`
  (`connector.no-channel`), and `Home` with a lease that is not longer than zero
  (`connector.lease`). `Retry`, with the hub error: `Remote` and `Mesh` of each, since
  the home can move back, and a mesh that stops ends the run at the cancel after at most
  one backoff. `Device` is for the device or the endpoint, never the hub. An error that
  the open never gives panics with `invariant:`: writer `Home` `Unsynced`, because
  `Hub::writer` waits for mesh time; reader `Unsynced`, because the supervisor opens a
  reader only after the status channels, which wait for mesh time, and mesh time stays
  once known; and reader `Transport`, `Refused`, `Message`, and `Pool`, because a named
  reader opens only at its home (`laptop.architect-2`, 2026-10-10T22:38:41Z:
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6102937379). The
  writer `Home` list and the codes: proposed in the plan on #1734
  (https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6104945733),
  approved by `laptop.architect-2` (2026-10-11T03:21:41Z,
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6104976908). The
  reasons: `laptop.architect` (2026-10-11T03:41:31Z,
  https://github.com/synnaxlabs/foundation/pull/2348#issuecomment-6105112575).
  A kind returns `Retry` when a session of it ends because the spec removed one of its
  channels (`hub::reader::Ended::Removed`, `hub::writer::Failure::Removed`): the next
  start opens the session again from its names or its selector, so it takes a channel
  that the spec defined again with a new key, and gets the open errors above when the
  channel is gone (`laptop.architect-2`, 2026-10-12T20:15:39Z:
  https://github.com/synnaxlabs/foundation/issues/2442#issuecomment-6131873162).
  A kind returns each end of its reader and each failed write with `?`:
  `From<hub::reader::Ended>` and `From<hub::writer::Failure>` for `kind::Error` give
  the class, each in one match with no `_` arm. `Retry`: reader `Behind`, `Removed`,
  `Replaced`, `Buffer`, and `Pool`, and writer `Removed` and `Home` `Full` and `Disk`.
  A reader at its home ends `Pool` when a read from disk finds no block, and blocks
  come back as the sessions of the shard drain (#274 B1; `laptop.architect-2`,
  2026-10-15T18:05:36Z:
  https://github.com/synnaxlabs/foundation/pull/2723#issuecomment-6198895692, finding
  1).
  `Config`: writer `Home` `Large` (`connector.large-frame`, with the fix "Give the
  connector fewer channels"), because only a spec change or a kind that splits its
  frames fixes it. A panic with `invariant:`: reader `Stream`, `Refused`, `Message`,
  `Frame`, and `Credit`, because a named reader opens only at its home, and
  writer `Home` `Resend`, because a kind labels no frame resend. A kind that wants
  another class matches the end before `?` (the `?`, the two `From` impls, and the
  match with no `_` arm: `laptop.architect-2`, 2026-10-12T20:24:54Z:
  https://github.com/synnaxlabs/foundation/issues/2489#issuecomment-6132012409, items
  1 and 2; the classes: `laptop.architect-2`, 2026-10-13T00:57:58Z:
  https://github.com/synnaxlabs/foundation/issues/2489#issuecomment-6135722511).
  A kind returns each failed draft of a frame with `?`: `From<frame::Error>` for
  `kind::Error` gives the class, in one match with no `_` arm. `Config`: pool
  `TooLarge` (`connector.large-frame`, with the fix "Give the connector fewer
  channels"), because only a spec change or a kind that splits its frames fixes it.
  `Retry`: pool `Exhausted` and `Refused`, as writer `Home` `Full`. A panic with
  `invariant:`: `OutOfRange`, `Unordered`, and `IndexAbsent`, because a kind gives its
  series from its own code, and `End`, which a draft never gives (the classes:
  `laptop.architect-2`, 2026-10-13T06:23:18Z:
  https://github.com/synnaxlabs/foundation/issues/435#issuecomment-6139612819, item 5;
  `From<frame::Error>` and the panics: `laptop.architect-2`, 2026-10-13T07:43:02Z:
  https://github.com/synnaxlabs/foundation/pull/2561#issuecomment-6140788066; the
  text: `laptop.architect`, 2026-10-13T07:42:14Z:
  https://github.com/synnaxlabs/foundation/pull/2561#issuecomment-6140774870).
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
