- **FRONT ENDS (#337, 2026-10-08)** `ops` takes a table of front ends from `node`, as
  it takes `kinds`, and does not depend on `config-hcl` (K1). `ops::FrontEnd { read:
  fn(Source, &str) -> Result<Document, Vec<Diagnostic>> }` is `Copy` with no
  `#[non_exhaustive]`, so `node` builds it with a struct literal. An error from `read`
  holds at least one problem. The table is `ops::FrontEnds`, which holds at least one
  front end, keyed by the extension with no dot (`"hcl"`) (`laptop.architect-2`,
  2026-10-09T05:10:54Z,
  https://github.com/synnaxlabs/foundation/pull/2078#issuecomment-6074720909).
  `FrontEnd` and `FrontEnds` have a hand-written `Debug` (`FrontEnd { .. }`,
  `FrontEnds(["hcl"])`), because a derive prints the address of `read`, which changes
  from run to run (`laptop.architect-2`, 2026-10-10T15:29:33Z:
  https://github.com/synnaxlabs/foundation/pull/2252#issuecomment-6099106464).
  Supersedes the derived `Debug` of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6054035444 and
  https://github.com/synnaxlabs/foundation/pull/2078#issuecomment-6074720909.
  Supersedes "gets `front_ends: BTreeMap<&'static str, FrontEnd>`" of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6054035444. The
  text after the last `.` of a file name picks the front end. A file with no front end
  gives `ops.unknown-extension` at the empty span at the start of the file, as
  DIAGNOSTICS says for a problem with a whole file: the message is "no config syntax
  reads this file", and the fix names each extension (`laptop.architect-2`,
  2026-10-08T18:11:55Z,
  https://github.com/synnaxlabs/foundation/pull/1950#issuecomment-6066159732).
  Supersedes "with no span" and "The message names the path" of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6054035444. A config
  path of `plan`, `browse`, or `import` that is a directory (not of `apply`, which reads
  one plan file) gives each file in it that the table reads, in name order, one level,
  with each name that starts with `.` skipped, and `ops.no-config` when it gives none.
  The filter runs on the bytes of a name before the UTF-8 check, so only a kept entry
  that is not UTF-8 gives `ops.path-not-utf8`. A kept entry that is a directory gives
  `ops.read`. Supersedes "in path order" of
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6054035444. The
  command line takes its table from `node::front_ends`, as
  `Config::front_ends` does (plan of PR 3,
  https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6253461918, revised
  in https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6253590781,
  approved by `laptop.architect-2`, 2026-10-18T18:23:36Z,
  https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6253485402, and for
  `browse` and `import`, 2026-10-18T18:34:45Z,
  https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6253598063; key
  choice accepted by `laptop.monitor`, 2026-10-18T18:21:43Z,
  https://github.com/synnaxlabs/foundation/issues/2526#issuecomment-6253466297). Lost:
  the node filters (the client sends each other file), recursion (a tree holds `.git`
  and editor files; trigger: a user who asks for a tree), and an empty config for an
  empty directory (it plans the removal of each entry of the node). `Source(i)` is
  the index of the file in the order `ops` reads it, and `ops` keeps the paths to
  print spans. `node` fills the table (#1756) with `config_hcl::read` (HCL
  DIAGNOSTICS). `write` and `update` join `FrontEnd` with the first operation that
  writes a file. Lost: `ops` calls `config_hcl::read` (breaks K1), a `FrontEnd` trait
  (one implementation, no state), and `Box<dyn Fn>` (no front end needs state).
  Decided by `laptop.architect-2` (2026-10-08T06:38:41Z,
  https://github.com/synnaxlabs/foundation/issues/337#issuecomment-6054035444).
