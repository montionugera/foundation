- **SECRET SOURCES** A config names where the value of a secret comes from, never the
  value: `env("NAME")`, `file("/absolute/path")`, or `secret("full.name")`. `secret()`
  also takes its name as a reference, `secret(full.name)`, as each name in a config does
  (HCL REFERENCES). `secret::Source::read` reads each attribute that takes a secret,
  once a kind reads one (#2583). A bare string, another function, a count of arguments
  other than 1, an argument that is not a string, an empty variable name or one with `=`
  or NUL, a relative `file()` path or one with a `..` segment, and a bad `secret()` name
  are `secret.source`. Its text never quotes what the user wrote, since that can be a
  pasted token. `Source` displays its call with the argument as a string, such as
  `secret("full.name")`, so `plan` and `status` show the source and never the value. The
  person decided this on 2026-10-13T20:59:00Z: "Yeah that's fine", to the three forms
  that `laptop.monitor` gave in
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155082412. It
  supersedes the bare secret name of K4 and Q5 of #2583. `laptop.architect-2` approved
  the reference form on 2026-10-13T22:18:44Z
  (https://github.com/synnaxlabs/foundation/pull/2619#issuecomment-6156410171). Decided
  by `laptop.architect-2` on 2026-10-13T21:05:46Z
  (https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6155189112).
