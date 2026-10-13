- **SECRET STORES AS ADAPTERS** `ctx.secret(name)` is the only path for kinds. Adapters:
  the built-in sealed store (default, works offline), a node environment variable or
  file, HashiCorp Vault, AWS Secrets Manager, Azure Key Vault, GCP Secret Manager, and
  Kubernetes Secrets. A policy picks the store per name. External adapters authenticate
  with the node key and may cache values sealed to it (this delays revocation).

  The first store is a file store, and every name resolves through it until the policy
  of #239 (ruling in the body of #2583, approved by the person on 2026-10-13 at
  16:24:35Z, relayed by `laptop.monitor`:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150321949; plan
  approved by `laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150199760
  (2026-10-13T16:26:56Z)). It supersedes "the built-in sealed store (default)" for
  connectors until #239. On `os`, the value of `plant.influx_token` is the file
  `<--data>/data/secrets/plant.influx_token`, read at each get, less one trailing `\n`
  or `\r\n`, at most 64 KiB. An empty value is missing. The store refuses a name with an
  ASCII capital, so no two names share a file on a file system that ignores case.
  `ctx.secret` gives a name that the store cannot hold (the store error `BadName`) as
  `Error::Config`, code `connector.bad-secret-name`, with the store's text as the fix
  (`laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6150818974). It gives
  each other store error as `Error::Retry`, with a text that names the secret and holds
  the store's text, which says where to put a missing value, as the full path of the
  file (`laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6150793141), so a
  value added heals the run at its next restart. Each get reads into a pool on
  `block::Erased`, which writes zero at the drop over each range that the pool
  committed, also after a failed or dropped read (`laptop.architect`:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6151055798,
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6151395377, and
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6153570364). The
  erase is at most 132 KiB and 64 bytes, for a file of 64 KiB. The allocation is the
  reservation of the pool, 5.2 MiB for that file. A get holds at most 5.4 MiB of heap
  at once, by the counting allocator, until it returns, or until the read of a dropped
  get ends. The value is at most 64 KiB while a kind holds it. Both are outside the
  pool budget of the node. The store does not check the owner or mode of the file.
  Deferred; trigger:
  `env::files` gives the mode of a file. The person has not yet answered this question:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150297190. The
  refusal of capitals and what a file holds are architect calls (`laptop.architect-2`,
  2026-10-13T16:54:37Z:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150705009).
