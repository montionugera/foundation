- **SECRET SOURCES** A config names where the value of a secret comes from, never the
  value: `env("NAME")`, `file("/absolute/path")`, or `secret("full.name")`. `secret()`
  also takes its name as a reference, `secret(full.name)`, as each name in a config does
  (HCL REFERENCES). `secret::Source::read` reads each attribute that takes a secret,
  such as `token` of the `influx` kind. A bare string, another function, a count of
  arguments other than 1, an argument that is not a string, an empty variable name or
  one with `=` or NUL, a `file()` path that does not start with `/`, that has NUL, or
  that has a `..` segment between `/` or `\`, and a bad `secret()` name are
  `secret.source`. Its text
  never quotes what the user wrote, because that can be a pasted token. `Source`
  displays its call with the argument as a string, such as `secret("full.name")`, so
  `plan` and `status` show the source and never the value. The person decided this on
  2026-10-13T20:59:00Z: "Yeah that's fine", to the three forms that `laptop.monitor`
  gave in https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155082412.
  Supersedes the bare secret name of K4 (`docs/decisions/access/k4.md`), item 2 of the
  ruling in the body of https://github.com/synnaxlabs/foundation/issues/2583 ("The
  token is a secret name"), and Q5 of
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150705009, and
  the node environment variable or file adapters of SECRET STORES AS ADAPTERS
  (`docs/decisions/access/secret-stores-as-adapters.md`).
  `laptop.architect-2` approved the reference form and the display of the argument as a
  string on 2026-10-13T22:18:44Z
  (https://github.com/synnaxlabs/foundation/pull/2619#issuecomment-6156410171). The path
  rule is the same text rule on every host, because no node runs on Windows and a rule
  can widen later with no break. The trigger to take a drive letter is the first node
  that runs on Windows. Decided by `laptop.architect-2` on 2026-10-13T22:22:37Z
  (https://github.com/synnaxlabs/foundation/pull/2619#issuecomment-6156477687).
  `laptop.architect-2` approved the refusal of NUL in a path on 2026-10-13T22:29:57Z
  (https://github.com/synnaxlabs/foundation/pull/2619#issuecomment-6156590921). The rest
  was decided by `laptop.architect-2` on 2026-10-13T21:05:46Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155189112).

  `secret::Resolver` gives the value of a source on one shard. `env()` reads the
  environment of the node process, which `node` main reads once at start
  (`std::env::vars_os()`) and gives in `node::Config::vars`, since the `token` of the
  `influx` kind. `laptop.architect-2` moved this read to that PR on
  2026-10-14T03:52:12Z
  (https://github.com/synnaxlabs/foundation/pull/2662#issuecomment-6160488141). No code
  below `node` reads the OS environment.
  To change the value, restart the node. A value that is not UTF-8 gives its bytes.
  `secret::Source::File` holds an `env::files::Absolute`, the one type of the path rule
  above, and `file()` reads it through `env::files::Files::open_absolute`, as
  `laptop.architect-2` ruled on 2026-10-13T22:35:35Z
  (https://github.com/synnaxlabs/foundation/pull/2621#issuecomment-6156672291).
  `secret()` reads through the store (SECRET STORES AS ADAPTERS).
  `secret::Resolver::new` makes the one turn that `file()` and the store share on a
  shard: one read at a time, in the order they come, in the erased pool, at most 64 KiB,
  less one trailing `\n` or `\r\n`. So the memory bound of SECRET STORES AS ADAPTERS
  holds for both. A variable of more than 64 KiB is `secret::Error::Unavailable`.
  `laptop.architect-2` decided the turn in `Resolver::new`, the one `secret::Error`, and
  this bound on 2026-10-14T03:11:41Z
  (https://github.com/synnaxlabs/foundation/pull/2662#issuecomment-6160027911). An
  empty value, a variable that is not set, and a file that is not there are
  `secret::Error::Missing`, with a text that says where to put the value.
  `Context::secret(&Source)` gives each error as `Error::Retry` with the source in its
  text, such as `the secret env("INFLUX_TOKEN"): the environment variable INFLUX_TOKEN
  of the node is not set. Start the node with it set to the value`, other than `BadName`
  of the store, which stays `Error::Config`. The mode check of #2596 is for the store of
  `secret()` only, because Docker and Kubernetes mount a secret file with mode 0444 or
  0644. `plan` resolves each source of each connector that the planning node runs, with
  the same read, as `secret.unresolved` (a later step of #2583). A connector on another
  node is not checked at plan until #239. A person who can apply a spec can make the
  node read any variable of its environment and any file that its user can read. The
  trigger for a limit that the operator of the node sets at start, such as directories
  and variable names: a spec applied by a subject that does not run the node. Decided by
  `laptop.architect-2` on 2026-10-13T21:05:46Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155189112), with
  the surface that it approved on 2026-10-13T21:26:42Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155552036).

  A kind names `Source` and `Value` through `connector::secret`, which re-exports them
  from `secret`. So a `connector-<kind>` crate does not depend on `secret`, and does
  not see `Resolver`, the stores, or the seal, which no kind may call. Lost: `secret`
  in the `connector-<kind>` line of the crate map. Decided by `laptop.architect-2` on
  2026-10-14T04:40:48Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6161042898).
