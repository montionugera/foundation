- **SECRET STORES AS ADAPTERS** `ctx.secret(source)` is the only path for kinds, and a
  store gives the value of `secret()` (SECRET SOURCES). Adapters: the built-in sealed
  store (default, works offline), HashiCorp Vault, AWS Secrets Manager, Azure Key
  Vault, GCP Secret Manager, and Kubernetes Secrets. A policy picks the store per name.
  External adapters authenticate with the node key and may cache values sealed to it
  (this delays revocation). A node environment variable or file is a source of its
  own, `env()` or `file()`, outside the store (SECRET SOURCES).

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
  (`laptop.architect-2`, 2026-10-13T17:00:54Z:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6150818974).
  Supersedes point 1 and "each store error as `Error::Retry`" of condition 2 of
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150199760. It gives
  each other store error as `Error::Retry`, with a text that names the secret and holds
  the store's text, which says where to put a missing value, as the full path of the
  file (`laptop.architect-2`, 2026-10-13T16:59:28Z:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6150793141), so a
  value added heals the run at its next restart. Each get reads into a pool on
  `block::Erased`, which writes zero at the drop over each range that the pool
  committed, also after a failed or dropped read (`laptop.architect`:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6151055798,
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6151395377, and
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6153570364). The erase
  is at most 64 KiB and 128 bytes, for a file of 64 KiB: the first 64 bytes and the
  footprint of class 35 (65,600 bytes). The allocation is the reservation of the pool,
  5.2 MiB for that file. The store runs one get at a time, and the next get waits its
  turn in the order it came (`laptop.architect-2`, 2026-10-13T19:49:00Z:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6153847139), also
  behind a read of `file()` (SECRET SOURCES). When a policy chooses a store,
  `Resolver::new` takes the policy's choice and still makes each store that reads files
  from its own `Reads`, so that store shares the turn (`laptop.architect-2`,
  2026-10-14T03:11:41Z:
  https://github.com/synnaxlabs/foundation/pull/2662#issuecomment-6160027911). So the
  gets of one shard hold at most 5.4 MiB of heap at once, plus less than 1 KiB for each
  get that waits for its turn, and 5.4 MiB for each read that a dropped get left
  running, until the disk ends that read. Those of a node hold that bound times the
  shard count. The counting allocator in `crates/secret/tests/memory.rs` measures the
  heap on the `sim` driver (ip-172-31-11-216, Linux x86-64), with the future of each
  get. Peaks: 81,254, 321,878, and 5,606,318 bytes for one get of a file of 1, 4096, and
  65,536 bytes, and 5,606,950, 5,783,090, and 6,317,162 bytes for 2, 256, and 1024 gets
  at once of 65,536 bytes, half of them through `file()`. On the `os` driver
  (ip-172-31-11-216, Linux x86-64), one get of the file store of #2588, before the
  resolver, held 81,070, 312,988, and 5,474,548 bytes, with no future counted. A dropped
  get gives the turn on at its drop, and its pool stays until its read ends. The value
  is at most 64 KiB while a kind holds it. Both are outside the pool budget of the node.
  The store does not check the owner or mode of the file. Make the file readable only by
  the user that runs the node. The mode check is #2596 (`laptop.architect-2`,
  2026-10-13T19:53:57Z:
  https://github.com/synnaxlabs/foundation/pull/2588#issuecomment-6153933564, under the
  person's rule of 2026-10-13T19:52:40Z that each choice goes ahead unless the person
  objects:
  https://github.com/synnaxlabs/foundation/issues/1737#issuecomment-6153919476). The
  refusal of capitals and what a file holds are architect calls (`laptop.architect-2`,
  2026-10-13T16:54:37Z:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150705009).
