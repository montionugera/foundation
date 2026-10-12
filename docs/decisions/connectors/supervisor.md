- **SUPERVISOR** `supervisor::Supervisor` runs one connector in two steps:
  `Supervisor::open`, then `Opened::run`. It never starts a run before the last one
  returned, and none after a cancel. `open` resolves once the status channels and the
  reader opened, or once the first run cannot start: the kind is unknown, the config
  does not parse or check, the mesh stopped, the reader open failed, or `cancel` is
  cancelled. `open` runs no code of the kind's `run`, so the connector writes no frame
  before `Opened::run`. Dropped, the `Opened` closes the reader and writes no status. So
  `node` can open the reader of an out connector before an in connector starts. Before
  each run, also a run after a backoff, the supervisor opens the reader from the
  `Channels::reader` that `check` gave, and the run takes it through `Context::reader`.
  A failed open ends that run with the class of its error (KIND TABLE), and the kind's
  `run` is not called. A run whose reader ends with
  `hub::reader::Ended::Behind` returns `Retry`, and the frames between the two opens
  do not come again before #274 (`laptop.architect-2`, 2026-10-11T05:11:21Z:
  https://github.com/synnaxlabs/foundation/issues/2354#issuecomment-6105720439, items 1
  and 3; 2026-10-11T05:23:35Z:
  https://github.com/synnaxlabs/foundation/issues/2354#issuecomment-6105802513, items 2,
  3, and 5). The reader open waits while the mesh names no home for the index that it
  selects. A reader with a hold opens again at its held position while its index keeps
  its key, so it stays `Behind` at each open within its hold, and each close starts the
  hold again (HOME NAMED READERS). So a hold longer than the longest wait of the backoff
  never ends while the node runs with that spec. The reader then gets no frame again
  until #274 reads from disk (the test:
  https://github.com/synnaxlabs/foundation/issues/274#issuecomment-6106518626). The wait
  for a home, and that the supervisor does no more for a held reader before #274, are
  decided by `laptop.architect-2` (2026-10-11T07:34:44Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6106726413, questions
  1 and 2). The text from "A reader with a hold" to the #274 test is from
  `laptop.architect` (2026-10-11T07:34:36Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6106725343), and
  "while its index keeps its key" is from `laptop.architect` (2026-10-12T20:35:29Z:
  https://github.com/synnaxlabs/foundation/pull/2488#issuecomment-6132165963). Each run
  gets a child of the caller's token, which the supervisor cancels once the run returns
  or its future drops, so each task that the run spawned to wait on it ends with the run
  (`laptop.architect-2`, 2026-10-08T17:58:15Z:
  https://github.com/synnaxlabs/foundation/pull/1944#issuecomment-6065930789). A drop of
  the future of `Opened::run` cancels the run and does not wait for its tasks
  (`laptop.architect-2`, 2026-10-09T01:30:51Z:
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6072411231). At a
  spec change, `node` cancels the run of each connector that the change removes or
  changes, and starts the new run of a changed connector only after its old `run`
  returned. Each other connector keeps its run, and the shard keeps its supervisor
  (`laptop.architect-2`, 2026-10-09T05:10:43Z:
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6074718616).
  Supersedes "At a spec change, `node` cancels each run and awaits it before it starts
  the new supervisor" of
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6072411231.
  A connector is also changed when a status channel of it has another key than at the
  last apply, with an equal kind, config, and node: a spec that the node did not see
  removed the channel, and the status session of the run holds the old key. `node`
  takes the keys from the spec in use: the status index of the connector, and each
  channel on it. A data channel with a new key changes no connector: the session of
  the kind ends, and the next start opens it again from its names or its selector
  (KIND TABLE). Lost: the key of the status index alone, because a kind that changes and
  comes back keeps the index and gives each count a new key; the names of the kind's
  `check`, which `node` then runs a second time at each apply; an end of the run when a
  status write gives `Failure::Removed`, which comes only at the next status write and
  changes CONNECTOR STATUS; a key of the connector in the spec, a new field of `spec`
  and a new rule of the plan for what the status keys already tell; each spec in order,
  because a node that was down sees only the last one (`laptop.architect-2`, #2358,
  2026-10-11T21:09:12Z:
  https://github.com/synnaxlabs/foundation/issues/2358#issuecomment-6113779704; the
  words of the data channel sentence, 2026-10-12T20:24:56Z:
  https://github.com/synnaxlabs/foundation/pull/2488#issuecomment-6132012721). The
  lookup of these keys stays in `node`. Lost: `connector::status` gives the keys from
  the definitions, which makes `spec` a dependency of `connector` and of each kind for
  one caller, where a connector reads no spec and reaches its channels only through
  `hub`, by name. That the status channels are on one index is the contract of
  `status::channels`, which `node` and `config` both read. Trigger: a second caller of
  the lookup, or status channels on more than one index (`laptop.architect-2`, #2451,
  2026-10-12T16:05:03Z:
  https://github.com/synnaxlabs/foundation/issues/2451#issuecomment-6127961287). A
  connector that a change adds starts once no first open of an added connector is
  pending on the node (below), unless its name has a loop (below): then the addition is
  a change, and the connector starts in that loop after the last run of its name
  returned, so no two runs write one set of status channels. A change of a connector
  whose run waits changes the connector that the run starts with, so a name has at most
  one run that waits. This supersedes "A connector that the change adds: start its run
  at once" of item 3 of 6074718616 (`laptop.architect-2`, 2026-10-10T15:29:59Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6099110161). The wait
  for each first open supersedes "starts at once" of the record text that that comment
  approved (`laptop.architect-2`, 2026-10-11T05:23:35Z:
  https://github.com/synnaxlabs/foundation/issues/2354#issuecomment-6105802513, item 4).
  The loop as the key supersedes "unless the last run of its name has not returned" of
  that text (`laptop.architect-2`, 2026-10-11T12:42:00Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109132275). While a
  first reader open waits for a home, no first run starts on the node
  (`laptop.architect-2`, 2026-10-11T07:34:44Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6106726413, question
  2). The open waits on the state of the mesh of this node. At each open of the mesh,
  that state starts at the founding, and it moves on only after the node hears a leader
  (MESH SURFACE). At a start in a region with no quorum, an out connector whose index
  has no founding home holds each connector of the node. It does so also when the
  region gave that index a home before the stop. The hold ends when the node hears a
  leader and applies the change that gives the home (#2380). #1770 makes the node apply
  its stored log at the open. Then only a home that the node never applied holds them.
  In a region of one node, the node leads and applies its log with its own vote
  (`laptop.architect`, 2026-10-11T12:28:57Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109025201). So
  `node` holds one count of the first opens of the connectors that each apply adds,
  pending from that apply until `Supervisor::open` resolves, and starts the first run of
  such a connector only once the count is 0. A changed connector, and one that runs
  again after `Config`, has no share and does not wait (`laptop.architect-2`,
  2026-10-11T05:23:35Z:
  https://github.com/synnaxlabs/foundation/issues/2354#issuecomment-6105802513, item 4).
  A connector that changes before the first run of its loop started is still an added
  connector: its next open is a first open, pending from the apply of the change until
  `Supervisor::open` resolves, and its first run waits for a count of 0. The change
  starts no run, so no old run holds that open. The first run of a loop starts at its
  first `Opened::run` that no change cancelled, also when that call gives `Config`
  (`laptop.architect-2`, 2026-10-11T13:01:50Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109296962). Lost: a
  share that the loop takes at its next poll after the change (`36da82ebe`): a first
  run can start between the apply and that poll (`laptop.architect-2`,
  2026-10-11T13:01:50Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109296962, item 2).
  `node` keeps one loop for each name, until the loop reads the removal of its connector
  after its last run returned. An addition of a name that has a loop is a change. So the
  first open of an out connector that an apply adds resolves before the first run of an
  in connector that this apply or a later one adds starts. A changed out connector opens
  its reader again only after its old run returned, so an in connector that its apply
  adds can write before that open, and those frames do not come again before #274
  (`laptop.architect-2`, 2026-10-11T12:26:28Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109004938). Lost: a
  share from the apply for a changed connector, or for a name added again, after the
  first run of its loop started. Its open starts only after its old run returned, so a
  task that does not end at the cancel then holds each first run on the node
  (`laptop.architect-2`, 2026-10-11T12:32:32Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109054993;
  2026-10-11T13:01:50Z:
  https://github.com/synnaxlabs/foundation/pull/2398#issuecomment-6109296962). After a
  run returns, the supervisor waits, with no timeout, until each task that the run
  spawned through `Context::tasks` ended, and only then starts its backoff. A task that
  does not end at the cancel is a defect of its kind (`laptop.architect-2`,
  2026-10-08T19:04:15Z:
  https://github.com/synnaxlabs/foundation/pull/1944#issuecomment-6067043337). It also
  waits so before it returns (`laptop.architect-2`, 2026-10-09T01:06:45Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6072159701, item
  1). The wait does not count toward the run's length (`laptop.architect-2`,
  2026-10-09T01:56:56Z:
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6072678811). One
  supervisor runs on each shard, made from `supervisor::Config` (the kinds, clock,
  entropy, network, tasks, and the shard's hub) (`laptop.architect-2`,
  2026-10-08T03:05:58Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6051331538). Only
  a shard that has a hub runs one, so today only shard 0 (`laptop.architect`,
  2026-10-10T14:41:53Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6098661378;
  `laptop.architect-2`, 2026-10-10T15:29:59Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6099110161). The
  tests of `connector` and of each kind build that config with
  `connector::testing::create_config`, behind `sim`, which opens the hub through
  `hub::testing::open`. It gives no mesh time, since a task gets it from its writer
  (#2143) (`laptop.architect-2`, 2026-10-09T13:37:08Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6082035824;
  `laptop.architect`, 2026-10-08T17:50:53Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6065807610). After
  `Device` or `Retry` it restarts with full jitter backoff (1 s first, 60 s cap,
  constants). The waits start again from 1 s after a run that lasted at least 60 s.
  `Ok` from `run` ends the connector.
  `Config` returns to the caller, which calls `run` again at the next spec change
  (R12-4) (`laptop.architect-2`, 2026-10-11T03:35:05Z:
  https://github.com/synnaxlabs/foundation/issues/1734#issuecomment-6105068890).
  Supersedes "when the spec changes the connector" (`laptop.architect-2`,
  2026-10-09T05:10:43Z:
  https://github.com/synnaxlabs/foundation/pull/2056#issuecomment-6074718616).
  A spec change that the caller applied after that run started counts, so one that
  comes while the run ends is not lost (`laptop.architect-2`, 2026-10-11T05:17:07Z:
  https://github.com/synnaxlabs/foundation/pull/2348#issuecomment-6105759604).
  Supersedes "The caller (`node`) already watches the spec and starts a new supervisor
  on a change (R12-4)" of the plan on #338
  (https://github.com/synnaxlabs/foundation/issues/338#issuecomment-5994872059). The
  class and the text of each restart error reach the connector's status (CONNECTOR
  STATUS).
  Decided by the `connector` builder in the plan on #338, after `/eb-review`; approved
  by the coordinator (#338), with the reset after a long run approved on #338 later.
- **CONNECTOR STATUS** The supervisor writes the status channels of its connector,
  `<connector>.status.<name>`, on their own index `<connector>.status.time`, as the
  connector, at `Authority::ABSOLUTE` (the advisor on #455, 2026-10-06T02:20:11Z:
  https://github.com/synnaxlabs/foundation/issues/420#issuecomment-6008006459), with
  no lease (`laptop.architect-2`, 2026-10-09T18:46:26Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6087144669). The
  index is homed on the connector's node. `connector::status::channels` gives each name
  and sample type:

  | Channel | Type | Values |
  | --- | --- | --- |
  | `state` | `u8` | 0 running, 1 waiting to restart, 2 stopped, 3 ending |
  | `class` | `u8` | the end of the last run: 0 none or `Ok`, 1 `Config`, 2 `Device`, 3 `Retry` |
  | `restarts` | `u64` | the restarts in this call |
  | `backoff` | `Span` | the wait from this sample to the next try, 0 unless `state` is 1 |
  | `error` | `String` | the text of the error that ended the last run, empty when `class` is 0 |
  | each count of the kind | `u64` | as the kind sets it through `Context::count` |

  `error`, and `backoff` but for the end of its wait (below), follow the #1735 ruling
  (`laptop.architect-2`, 2026-10-08T06:29:21Z:
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186). The
  text of `error` is the text of the `Device` or `Retry` source, or the diagnostics of
  `Config` joined with `"; "`, cut to at most 1024 bytes at a char boundary, so a long
  device error does not make a frame too large. `backoff` is the wait that is left to
  the next try by the node's clock at the write, less the time by which the frame is
  stamped after the hub's time (below). So a frame written again later gives the wait
  that is left, a frame stamped ahead gives a wait that ends at the next try, and a step
  of the hub's time during the wait does not change it. The wait is by the node's
  clock, so while the hub's time slews, the next try is up to 500 ppm (MESH SLEW) of
  the wait from the frame's time plus `backoff`. Decided by the `connector` builder in
  the plan on #420 (2026-10-10T03:22:36Z:
  https://github.com/synnaxlabs/foundation/issues/420#issuecomment-6093259074),
  measured from the frame's time after round 1 of PR #2227 (finding 2,
  2026-10-10T03:55:58Z:
  https://github.com/synnaxlabs/foundation/pull/2227#issuecomment-6093511014), and by
  the node's clock after round 2 (finding 1, 2026-10-10T05:36:36Z:
  https://github.com/synnaxlabs/foundation/pull/2227#issuecomment-6094269805). The
  `backoff` text is approved by `laptop.architect` with "next run"
  (2026-10-10T05:38:31Z:
  https://github.com/synnaxlabs/foundation/pull/2227#issuecomment-6094282904) and with
  "next try" (2026-10-11T10:03:33Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6107853141). The cut
  and the join are approved by `laptop.architect-2` (2026-10-10T04:00:21Z:
  https://github.com/synnaxlabs/foundation/pull/2227#issuecomment-6093545130). Each run
  opens its reader before its start status (`laptop.architect-2`, 2026-10-11T07:34:44Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6106726413). A try is
  the reader open of a restart, or, when `check` gives no reader settings, its start
  status. So while that open waits for a home, `state` stays 1 with a `backoff` whose
  wait has ended. That `backoff` ends at a try is decided by `laptop.architect-2`
  (2026-10-11T10:17:43Z:
  https://github.com/synnaxlabs/foundation/pull/2371#issuecomment-6107989901). It
  supersedes "the wait from this sample to the next run" of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6053869186, and "the
  time of the next run" of the plan on #420
  (https://github.com/synnaxlabs/foundation/issues/420#issuecomment-6093259074).

  Each write is one frame with the last value of every status channel. Each start of a
  run writes the whole status. A change of `state`, `class`, `restarts`, `backoff`, or
  `error` is written as soon as the home applied the state before it (below). A change
  of counts alone is written at most once each second after the last write, timed with
  the clock of `supervisor::Config` (`laptop.architect-2`, 2026-10-08T06:38:42Z:
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6054035730, rules 1
  to 3). When a run returns, `state` 3 with the class of that end is written as soon as
  the home applied the state before it. When each task of the run ended, `state` 1, or 2
  when the call returns
  (`laptop.architect-2`, 2026-10-08T19:07:43Z:
  https://github.com/synnaxlabs/foundation/issues/420#issuecomment-6067099903). A run
  whose last task ends T after its start gives at most 1 + ⌊T / 1 s⌋ + 2 frames: the
  start, at most one frame of counts in each second, then `state` 3 and the next state.
  So a kind that counts 10,000 samples over 10 s gives at most 13. When the run itself
  sets its last count and ends at the flush at 10 s, that flush writes no frame, and it
  gives 12. T runs to the end of the last task, because a task that outlives the run
  can set counts between `state` 3 and the next state (`laptop.architect-2`,
  2026-10-09T21:16:18Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6089396653).
  Supersedes the bound of 12 frames of
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088160535.
  `state` 3 and the next state stay two frames also when no task is left, because
  `state` 3 marks the end of the run (`laptop.architect-2`, 2026-10-09T19:51:38Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088160535).
  `status::Writer` holds the rules of each channel but the counts, and the
  supervisor calls its `start`, `end`, `wait`, and `stop` (same ruling, for `class`
  and `restarts`; for `backoff` and `error`, the #1735 ruling and the plan on #420
  above). A frame that
  the home does not apply (`Waiting`, `Reserved`, `Order`, or `Lost`), or for which the
  shard's pool has no block now (`block::Error::Exhausted` or `Refused` in
  `frame::Error::Pool`), leaves the status staged, so it is written again one second
  later, also the last frame of a call: the call returns once the home applied it, or
  once `cancel` is cancelled. The pool case is load, not a defect
  (`laptop.architect-2`, 2026-10-09T21:32:53Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6089614613). A
  status frame larger than the largest block of the pool (`block::Error::TooLarge`) is
  a defect, and panics (`laptop.architect-2`, 2026-10-09T21:40:46Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6089720287). A
  change of state waits until the home applied the state before it, or until `cancel`
  is cancelled, so no state replaces a state that the home did not apply. A start
  whose wait a cancel ends writes nothing, and no run starts. After
  `Failure::Removed` or `home::Error::Disk`, no change of state waits
  (`laptop.architect-2`, 2026-10-09T21:46:07Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6089790301).
  Supersedes "at once" in rule 3 of
  https://github.com/synnaxlabs/foundation/issues/1735#issuecomment-6054035730 and
  "The frame's time is when the wait started" in
  https://github.com/synnaxlabs/foundation/issues/420#issuecomment-6067099903: the
  time of a frame is the time of the hub at its write, or the nanosecond after the
  last stamp of the writer when that is later (`laptop.architect`,
  2026-10-09T23:05:03Z:
  https://github.com/synnaxlabs/foundation/pull/2173#issuecomment-6090762715). After
  `Failure::Removed` or `home::Error::Disk` the call writes no more status. Each other
  refusal is a defect of `connector`, and panics (`laptop.architect-2`,
  2026-10-09T19:41:05Z:
  https://github.com/synnaxlabs/foundation/pull/2173#issuecomment-6087998186). While
  the shard's pool gives no frame, a change of state waits also after its status
  channels were removed or its disk failed, until the pool frees or `cancel` is
  cancelled (`laptop.architect-2`, 2026-10-09T23:16:59Z and 2026-10-09T23:33:09Z:
  https://github.com/synnaxlabs/foundation/pull/2173#issuecomment-6090887547 and
  https://github.com/synnaxlabs/foundation/pull/2173#issuecomment-6091053596). A frame
  that the home refuses as `Backwards` is stamped again after the stamp that the
  refusal gives, and written again at once, one time. A writer starts with no last
  stamp, so its first frame can be at or before the last frame of the writer before it
  on the same index (`laptop.architect-2`, 2026-10-09T20:20:46Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088599351). Each
  other refusal of the frame written again follows the rules above. A second
  `Backwards` is a defect, and panics (`laptop.architect-2`, 2026-10-09T20:24:02Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088648651). The
  status channels open once the node has mesh time, and no run starts before. A cancel
  while they wait to open gives `Ok` at once (`laptop.architect-2`,
  2026-10-09T19:43:41Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088037219, item 4).
  The tests of that wait open the hub with `hub::testing::open_unsynced`, whose mesh
  time starts after a delay (`laptop.architect`, 2026-10-09T19:41:28Z:
  https://github.com/synnaxlabs/foundation/pull/2173#issuecomment-6088003915). The
  status writer that does not open panics on an unknown or remote channel, which is a
  defect, because the plan homes the index on the connector's node (above), and gives
  `Ok` when the mesh stopped (`laptop.architect-2`, 2026-10-09T18:46:26Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6087144669). Until
  #2337, an apply that moves a connector to another member keeps its status index and
  the home of that index (S12), so its run on the new member panics
  (`laptop.architect-2`, 2026-10-11T01:46:50Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6104325892). An
  unknown channel after the cancel of the call also gives `Ok`, because `node` removes
  the status channels of a connector before it cancels its run. This supersedes "a panic
  is right" of item 2 of 6087144669 for `Unknown` in a cancelled call
  (`laptop.architect`, 2026-10-10T15:07:08Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6098894431;
  `laptop.architect-2`, 2026-10-10T15:29:59Z:
  https://github.com/synnaxlabs/foundation/pull/2261#issuecomment-6099110161).
  `status::channels` replaces the public `status::TIME` and `status::CHANNELS`, so one
  call gives each name and type (ruling 6087144669). The tests define the status
  channels with `connector::testing::create_status`, behind `sim`, which gives their
  definitions with keys from a `channel::Key` on. `sim` also turns on the optional
  dependency on `spec`, whose `Definition` the helper gives. Lost: a helper that calls
  `Hub::set_definitions` itself, because each call replaces all definitions and a test
  sets its own in the same call (`laptop.architect-2`, 2026-10-09T19:04:30Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6087430425).
  `connector::status::channels` gives `types::name::Error::Long` for the first status
  name over `Name::MAX_BYTES`, because the connector's name comes from the user's file.
  `config::plan` gives that error as a diagnostic at the connector's label (#1821).
  `Writer::open` panics on it, because the plan refused the name. Lost: a public
  constant of the longest status suffix. The count rules stay in `kind::Table::check`,
  which has the kind's name for the message, and `channels` takes `counts` as `check`
  gives them. A kind gets a count through `Context::count`, and `status::Status` stays
  inside the crate (`laptop.architect-2`, 2026-10-09T19:43:41Z:
  https://github.com/synnaxlabs/foundation/issues/1731#issuecomment-6088037219, items 1
  to 3).
