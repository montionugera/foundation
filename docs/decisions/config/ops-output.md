- **OPS OUTPUT (#337, 2026-10-08)** The plan output is a typed `plan::Output { base,
  changes, homes, added, changed, removed }`, with `Change { action, kind, name, place:
  Option<Place> }`. It derives `Serialize`, `Deserialize`, and `JsonSchema`, and `ops`
  resolves each span to a file, line, and column. `text()` gives the terminal text, as
  `Response` and `Error` do. The CLI writes its `text()`, or its JSON with `--json`
  (`laptop.architect-2`, 2026-10-13T08:24:09Z,
  https://github.com/synnaxlabs/foundation/pull/2528#issuecomment-6141442842, which
  supersedes "#1744 makes it a `Response` variant" of
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6064364728). Each
  error of `ops`, in the CLI and in MCP, has one JSON shape: `{"errors": [...]}`, each
  item a typed `Problem { code, message, fix, place: Option<Place>, notes }`. An error
  that is not a diagnostic, such as `ops.argument`, is a list of one item with no place.
  A place is `{"file", "line", "column"}`, each count from 1, and the key is absent when
  there is no place. `notes` is a list, empty when there is no note. The text of an
  error is each item as `error[<code>]: <message>`, its place, `fix: <fix>`, and each
  note, with an empty line between two items. Lost: a second shape for diagnostics, with
  which a client parses two shapes and must know which operation gives which. Decided by
  `laptop.architect-2` (2026-10-08T16:26:42Z,
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6064364728). The
  place and notes details: `laptop.architect-2` (2026-10-08T16:40:27Z,
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6064603605).
  A subject change has `fingerprints`: the `SHA256:` fingerprint of each key, as
  `ssh-keygen -l` writes it, sorted by the bytes of the key, after the apply, or before
  it for a removal. The key is absent for each other kind. The text gives one line
  `    key SHA256:...` for each, under its change. Lost: a diff of the keys of a change;
  a reviewer must trust the end state, and the full list shows each key it grants.
  Trigger: `fingerprints` is the only field of `Change` for one kind. When a second
  kind adds a field of its own, those fields become one tagged `detail` enum. Decided
  by `laptop.architect-2` (2026-10-08T17:21:06Z,
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6065299343).
  `config` owns the OpenSSH form, so `config::openssh::fingerprint(PublicKey) ->
  String` makes it, and `ops` has no `ssh-key`. Lost: `ssh-key` in `ops`, a second
  owner of the form. Decided by `laptop.architect-2` (2026-10-08T17:56:21Z,
  https://github.com/synnaxlabs/foundation/pull/1943#issuecomment-6065898724).
  Supersedes the `ops` dependency on `ssh-key` in item 3 of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6065299343.
  `plan` refuses a path that is not UTF-8 with `ops.path-not-utf8`, before its
  extension, so `Place::file` is the exact path. `ops.path-not-utf8` has no span,
  since a place cannot hold the path, and writes it with `{path:?}` until #941.
  `ops.unknown-extension` gets a place and names no path in its message (FRONT ENDS),
  so its path is exact too (`laptop.architect-2`, 2026-10-08T18:11:55Z,
  https://github.com/synnaxlabs/foundation/pull/1950#issuecomment-6066159732).
  Supersedes
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6066070872.
  Supersedes item 4 of
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6066009681.
  Lost: the lossy text, with which two files give one place; the `Debug` form in
  `Place::file`, which each JSON reader must decode; and the escaped path in the
  message of `ops.unknown-extension`, which an agent must decode. The `Place::file` doc
  names the code, not the function that refuses the path (`laptop.architect-2`,
  2026-10-08T18:12:57Z,
  https://github.com/synnaxlabs/foundation/pull/1950#issuecomment-6066177474).
  Supersedes items 2 and 3 of
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6066009681.
  Decided by `laptop.architect-2` (2026-10-08T18:03:02Z,
  https://github.com/synnaxlabs/foundation/pull/1911#issuecomment-6066009681).
  The apply gives six codes of its own. A plan file path that is not UTF-8 is
  `ops.path-not-utf8`, status 2, as in `plan`. Each `config::plan::Error` is
  `ops.bad-plan`, status 2, with its `Display` as the message and the fix ``Make a plan
  with `foundation plan`, and apply it with no edits``. A node with no spec in use or
  with `Spec::behind` set gives `ops.behind`, status 1, after the decode and before the
  base compare: the message names the newest pointer and one line for its `Cause`, and
  the fix is `Fix the cause, then plan again`. One private function of `ops` reads
  `Mesh::spec` for each caller and gives the spec in use or the stop (`ops.stopped`). A
  call on that spec, the check that it is the newest, gives this error. This supersedes
  the one function of item 1 of
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071339913, which also
  gives `ops.behind`. The one reader and its newest check are by `laptop.architect-2`
  (2026-10-12T18:47:07Z, item 2 and the first text fix of
  https://github.com/synnaxlabs/foundation/pull/2466#issuecomment-6130500446) and, for
  `Mesh::spec`, by `laptop.architect` (2026-10-12T19:42:40Z,
  https://github.com/synnaxlabs/foundation/pull/2466#issuecomment-6131381280). A plan
  whose base is not the spec in use, before the proposal or after another change applies
  first, is `ops.stale-plan`, status 1, fix `Plan again`. Each other error of
  `Mesh::apply`, except a stop, is `ops.apply`, status 1, with its `Display` as the
  message and the fix `Fix the cause in the message, then plan and apply again`. A stop
  of the group of the mesh, from `Mesh::spec` or from `Mesh::apply`, is `ops.stopped`,
  status 1, in `plan` and in `apply`, with the `Display` of the stop as the message and
  the fix `Fix the cause in the message, then start the node and plan again`. It can
  come after a proposal, so plan again to see whether the change applied. Lost: a code
  for each variant of the stop, which gives a better hint but adds three codes for one
  cause. The code has only this cause: a node that does not run gives
  `node.not-running` (below), which supersedes `ops.no-node` of
  https://github.com/synnaxlabs/foundation/pull/2192#issuecomment-6090851088.
  Supersedes the code `ops.stopped` of `Unreached::Stopped` in
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6053197935.
  `ops.stopped` decided by `laptop.architect-2` (2026-10-09T03:42:35Z, (b) of
  https://github.com/synnaxlabs/foundation/issues/1756#issuecomment-6073812317); the
  fix, which supersedes the fix of (b), by `laptop.architect` (2026-10-09T22:19:11Z,
  https://github.com/synnaxlabs/foundation/pull/2192#issuecomment-6090194371) and
  `laptop.architect-2` (2026-10-09T22:24:28Z,
  https://github.com/synnaxlabs/foundation/pull/2192#issuecomment-6090256164), one
  code with this fix for each stop by `laptop.architect-2` (2026-10-09T23:12:10Z,
  https://github.com/synnaxlabs/foundation/pull/2192#issuecomment-6090836329), and
  "except a stop" by `laptop.architect-2` (2026-10-09T22:19:29Z,
  https://github.com/synnaxlabs/foundation/pull/2192#issuecomment-6090197852).
  Supersedes the code of `mesh::Error::Stopped` in step 4 of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6063695586. There
  is no `ops.reserved-change`: `Plan::definitions` refuses a change at a reserved label
  (FIRST ADMIN). After `definitions`, each problem of `config::plan::check` is a
  `Config` problem with its own code and no place, status 2 (PLAN FILE): plan
  https://github.com/synnaxlabs/foundation/issues/2013#issuecomment-6071877567, approved
  by `laptop.architect-2` (2026-10-09T00:48:39Z,
  https://github.com/synnaxlabs/foundation/issues/2013#issuecomment-6071969872).
  `ops.path-not-utf8` in the apply decided by `laptop.architect-2`
  (2026-10-09T20:50:27Z, item 1 of
  https://github.com/synnaxlabs/foundation/pull/2178#issuecomment-6089032114).
  `ops.behind` and the `ops.apply` fix decided by `laptop.architect-2`
  (2026-10-08T23:51:37Z, items 1 and 2 of
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071339913). The
  order and the codes `ops.stale-plan` and `ops.apply` are steps 3 and 4 of the plan
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6063695586, approved
  by `laptop.architect-2` (2026-10-09T00:07:17Z,
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071513636) and by
  `laptop.architect` (2026-10-09T00:29:47Z,
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071766367). The
  `ops.bad-plan` fix and the order of the base compare decided by `laptop.architect-2`
  (2026-10-08T22:57:59Z,
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6070706432).
  Supersedes the code, message, fix, and order of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6060043966.
  The apply output is `Applied { file, pointer, added, changed, removed, homes }`, where
  `homes` counts the homes of the plan that the apply gives: each home that the plan
  lists, and through `ops::serve` each home on whose index the subject has `apply`
  (SERVE ACCESS). Its text is `Applied <file>: <a> added, <c> changed, <r> removed, <h>
  homes listed.`, with each count of 0 left out and `1 home listed` for one home, and
  `no change` only for a plan with no change and no home. The text without `homes` and
  the rule that leaves out a count of 0 are step 5 of the plan
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6063695586. `homes`,
  its text, and the `no change` rule decided by `laptop.architect-2`
  (2026-10-09T00:42:24Z,
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071903259). The whole
  text approved by `laptop.architect-2` (2026-10-09T01:02:46Z,
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6072118033) and, for
  the contract with `mesh`, by `laptop.architect` (2026-10-09T00:57:47Z,
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6072065332).
  The plan output also has `reserved`: each definition of the spec in use that
  Foundation made, as `Reserved { name, kind, key }` with its label, its kind, and its
  tree key, in the order of its tree key. `plan` leaves these out of its changes, since
  no file can hold a reserved label (FIRST ADMIN). When the spec holds one, the text
  ends with one line after the counts: `note: <n> definitions that Foundation made are
  not in the files:` and the tree key of each, with `, ` between two, or `note: 1
  definition that Foundation made is not in the files:` and its tree key. A spec with
  none gives no line and an empty `reserved`. The text and the field are item 4 of plan
  revision 1 of #1744, by `laptop.architect-2` (2026-10-08T06:01:36Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6053458318), which
  revision 3 moves to PR 2 (2026-10-10T03:30:54Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6093320483). The
  order of the tree key is the ruling of `laptop.architect-2` (2026-10-12T18:27:00Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6130170904), which
  supersedes the order of the example in item 4. The field `key` is by
  `laptop.architect-2` (2026-10-12T18:47:07Z,
  https://github.com/synnaxlabs/foundation/pull/2466#issuecomment-6130500446).
  A remote program runs `plan`, `apply`, and `status` (STATUS OPERATION) on a node
  through `ops::serve`. The request form is private to `ops`: one line of JSON with one
  operation and its input, a newline, then the bytes of a plan file as they are. `plan`
  sends the path and the text of each file:
  `{"plan":{"files":[{"path":"site.hcl","text":"..."}]}}`, with no byte after the
  newline. `apply` sends `{"apply":{}}` and then only the plan. The command line checks
  the path and fills `Applied::file`, so the node sees no path and gives the apply
  output with no `file`. The reply is `{"output":<the output>}` and a newline, then the
  plan file for `plan`. The reply of an error is `{"errors":[...], "status":<n>}` and a
  newline, with the `errors` of each error of `ops` and the exit status that the error
  has (2 for a `Config` or a `Plan` error), not always 1. Three codes are new, each with
  status 1. A body that is not this form is `ops.bad-request`, with the cause in the
  message and the fix ``Use a `foundation` program of the same version as the node``. So
  is each operation other than `plan`, `apply`, and `status`, with the cause ``the
  operation `<name>` does not run on a node``. A subject with no grant gets `ops.denied`
  (SERVE ACCESS). A reply of more than `wire::hub::client::BODY_BYTES_MAX` bytes is
  `ops.large-reply`, with the bound in the message and the fix `Plan and apply the
  change in smaller steps`, since `hub::serve::Reply::send` panics on such a body
  (`laptop.architect`, 2026-10-08T11:24:14Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6058791545). The
  bound holds for the reply of an error too. `serve` stops writing a reply at the bound.
  The places of the problems of one file share its path (`laptop.architect`,
  2026-10-12T20:11:00Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6131805444;
  `laptop.architect-2`, 2026-10-12T20:14:30Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6131856346). `serve`
  reads the spec in use as the apply does, so a node that is behind gives `ops.behind`
  for `plan` too (later step of
  https://github.com/synnaxlabs/foundation/pull/2035#issuecomment-6071339913, in
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6071715327). A node
  that uses no spec gives `ops.denied` to each remote subject instead (SERVE ACCESS).
  The form, the status of an error, `ops.denied`, and `ops.large-reply` are decisions 5
  to 7 of plan revision 4, by `laptop.architect-2` (2026-10-11T07:10:55Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913),
  approved for the contracts by `laptop.architect` (2026-10-11T08:10:05Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106994274). The
  status of `ops.bad-request`, its fix, and its use for each operation other than `plan`
  and `apply` are by `laptop.architect-2` (2026-10-12T18:30:05Z,
  https://github.com/synnaxlabs/foundation/pull/2468#issuecomment-6130219053), which
  supersedes the `ops.unknown-operation` clause of the `serve` doc of that plan. STATUS
  OPERATION adds `status` to `plan` and `apply` there. Lost: one JSON value with the
  plan file as a list of numbers, which is four times the bytes.
  `plan`, `apply`, and `status` (STATUS OPERATION) run on the node of `foundation-data`,
  which `node::client::reach` reaches. The command line gives four codes of its own.
  `ops.read`: `no file is at <path>`, `<path> is a directory, not a file`, or `the text
  of <path> is not UTF-8`, status 2, and `the read of <path> failed with OS error
  <code>`, status 1, each with the fix `Fix the cause in the message, then run the
  command again`. `ops.large-request`: `the request has more than <n> bytes, the most
  that a request holds`, for config files whose bytes together pass that bound at their
  read, or for the encoded request, status 2, fix `Make the config files smaller`; the
  check comes before the reach. `ops.write`: `the directory of the plan file <path> is
  not there` or `the write of the plan file <path> failed with OS error <code>`, status
  1, fix ``Give `--out` a path in a directory that can be written``. `ops.bad-reply`:
  `the reply of the node is no reply: <why>`, status 1, fix ``Use a `foundation` program
  of the same version as the node``. A failure to reach the node, or of its request, has
  status 1 and one of the codes `node.not-running`, `node.unreadable`, `node.admin-key`,
  `node.socket`, `node.unanswered`, `node.session`, `node.refused`, and `node.no-reply`,
  with the message and fix of decision 3 of plan revision 4 (`laptop.architect-2`,
  2026-10-11T07:10:55Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913). The
  fix of `node.not-running` is ``Start one with `foundation start --name <name>` ``,
  with no `--data`, by decision 7 of plan revision 5
  (https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136638641),
  approved by `laptop.architect-2` (2026-10-13T02:10:54Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136677419).
  `node.session` is by `laptop.architect-2` (2026-10-13T01:44:19Z,
  https://github.com/synnaxlabs/foundation/pull/2517#issuecomment-6136254936).
  `node.socket` covers a socket of the program that broke, with the message `the
  socket of this program failed: <the transport error>` (`laptop.architect-2`,
  2026-10-13T02:10:54Z, item 4 of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136677419). The
  transport error in `node.unanswered` is by `laptop.architect-2` (2026-10-13T01:44:19Z,
  https://github.com/synnaxlabs/foundation/pull/2517#issuecomment-6136254936).
  Supersedes, in decision 3 of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913, the fix
  of `node.not-running`, the message of `node.socket`, the hub error in
  `node.unanswered`, and the example `read of admin.key` of `node.unreadable`, which the
  error of `env::files` gives as `read_at of admin.key failed with OS error 13`
  (`laptop.architect-2`, 2026-10-13T04:17:48Z, item 2 of
  https://github.com/synnaxlabs/foundation/pull/2533#issuecomment-6138124615). The
  four `ops` codes are decisions 2, 4, 5, and 8 of plan revision 5,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136638641, with
  `ops.large-request` by item 2 of the approval of `laptop.architect-2`
  (2026-10-13T02:10:54Z,
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136677419), which
  supersedes the bound on each file of that plan. `laptop.architect-2` approved the
  messages, statuses, and fixes of the four `ops` codes (2026-10-13T03:48:32Z, item 4 of
  https://github.com/synnaxlabs/foundation/pull/2533#issuecomment-6137791971).
  `plan`, `apply`, and `status` take `--data <dir>`, the data directory of the node, by
  default `foundation-data`, as `start` does (#2526). This supersedes "run on the node
  of `foundation-data`" above and in the doc of `cli` of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136638641. In each
  command of a fix below, `{dir}` follows `--data=` as one word of a POSIX shell: in
  single quotes, with each `'` as `'"'"'`, when it is empty or holds a character other
  than `[A-Za-z0-9_./:@%+,-]`. A `{dir}` that is not UTF-8, or that the escape of a fix
  text changes (a `\`, a control character, or another character that
  `char::escape_debug` escapes), is named in no command: the fix says to set `--data` to
  that directory. In a message, `{dir}` is the path as it is. The fix of
  `node.not-running` is ``Run `foundation start --data={dir}`, with `--name <name>` on
  its first start, or give `--data` the data directory of a node that runs``, which
  supersedes "with no `--data`" above, decision 7 of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6136638641. A
  `{dir}` that is a file, or whose `{dir}/data` is a file, is also `node.not-running`,
  with the message `no node runs in {dir}, which is not a data directory` and the fix
  ``Give `--data` the data directory of a node that runs``, which supersedes
  `node.unreadable` for it, "a directory that does not open" of decision 3 of
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913. The fix
  of `node.unanswered` is ``Start the node again with `foundation start --data={dir}`
  ``, which supersedes its fix of that decision 3. The fix of `node.session` is `Read the log
  of the node on its standard error, then run the command again`, which supersedes the
  fix of `node.session` of item 1 of
  https://github.com/synnaxlabs/foundation/pull/2517#issuecomment-6136254936, by the
  trigger of https://github.com/synnaxlabs/foundation/issues/2606#issuecomment-6155695712
  once the node writes its log (LOG OUTPUT). The fix of `ops.unknown-operation`
  with no close name names the list of its carrier: ``Use a name from `foundation
  --help` `` on the command line, and ``Use a name from `tools/list` `` in MCP, because
  `foundation docs` lists only the operations of `TABLE`. By `laptop.architect-2`
  (2026-10-13T21:30:55Z,
  https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6155625024; the fix
  of `ops.unknown-operation` at 2026-10-13T21:47:43Z,
  https://github.com/synnaxlabs/foundation/pull/2614#issuecomment-6155892894; the texts
  that name `{dir}` at 2026-10-13T21:52:28Z,
  https://github.com/synnaxlabs/foundation/pull/2614#issuecomment-6155978119; the
  quotes of `{dir}` and the text of a `{dir}` that is no data directory at
  2026-10-13T22:10:06Z,
  https://github.com/synnaxlabs/foundation/pull/2614#issuecomment-6156250837;
  `--data={dir}` at 2026-10-13T22:25:40Z,
  https://github.com/synnaxlabs/foundation/pull/2614#issuecomment-6156527941; and the
  quotes of `'`, the safe set, and the fixes that name no path at 2026-10-13T22:31:15Z,
  https://github.com/synnaxlabs/foundation/pull/2614#issuecomment-6156609096).
  `version`, `foundation --version`, and `-V` give one output: the text
  `<version> (<commit>)`, with the first 12 hex digits of the commit or `unknown`, and
  the JSON `{"commit": <40 or 64 hex digits, or null>, "version": <version>}`, so a
  program tests `null`, not a word. The commit is that of `HEAD` when the `build.rs` of
  `ops` runs, also in a tree with changes, with no mark of them, and is none with no
  `git`, outside a git repository, and on a branch with no commit. Only the script sets
  it, so a variable of the builder does not reach `ops`, and it stops the build with an
  error at a commit that is not 40 or 64 lowercase hex digits and at a ref of a branch
  with a commit that git cannot read. The script watches the paths that
  `git rev-parse --git-path` gives for `HEAD` and its ref, and, only while that ref has
  no file of its own, the nearest folder above the ref, where git writes that file at
  the next change of the ref. In a reftable repository, it watches in their place the
  `reftable` folder of the worktree, which holds its `HEAD`, and that of the repository,
  which holds each branch. So a commit, a checkout, and a branch switch in a worktree
  rebuild `ops`, and a build with no change does not. MCP `serverInfo` keeps the version
  alone (`laptop.architect-2`, 2026-10-13T21:55:47Z,
  https://github.com/synnaxlabs/foundation/issues/2616#issuecomment-6156028343; the
  check of the commit: `laptop.architect-2`, 2026-10-13T22:33:11Z,
  https://github.com/synnaxlabs/foundation/pull/2620#issuecomment-6156635866; the
  reftable folder, the empty commit, and the cases with no commit: `laptop.architect-2`,
  2026-10-13T22:39:49Z,
  https://github.com/synnaxlabs/foundation/pull/2620#issuecomment-6156732029; the commit
  of 64 digits: `laptop.architect-2`, 2026-10-13T23:24:59Z,
  https://github.com/synnaxlabs/foundation/pull/2620#issuecomment-6157329508; no watch
  of `packed-refs`: `laptop.architect-2`, 2026-10-13T23:38:27Z,
  https://github.com/synnaxlabs/foundation/pull/2620#issuecomment-6157497960). Lost:
  `vergen`, a new dependency for about 30 lines; a variable that the release sets, as
  each local build must say what it is; and a watch of `packed-refs` (git writes the
  file of a ref at each change of it).
