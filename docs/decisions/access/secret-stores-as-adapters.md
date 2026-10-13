- **SECRET STORES AS ADAPTERS** `ctx.secret(name)` is the only path for kinds. Adapters:
  the built-in sealed store (default, works offline), a node environment variable or
  file, HashiCorp Vault, AWS Secrets Manager, Azure Key Vault, GCP Secret Manager, and
  Kubernetes Secrets. A policy picks the store per name. External adapters authenticate
  with the node key and may cache values sealed to it (this delays revocation).

  The first store is a file store, and every name resolves through it until the
  policy of #239 (ruling in the body of #2583, approved by the person on 2026-10-13
  at 16:24:35Z, relayed by `laptop.monitor`; plan approved by `laptop.architect-2`:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150199760). The
  value of `plant.influx_token` is the file `secrets/plant.influx_token` of the data
  directory, read at each get, less one trailing `\n` or `\r\n`, at most 64 KiB. An
  empty value is missing. The store refuses a name with an ASCII capital, so no two
  names share a file on a file system that ignores case. `ctx.secret` gives each
  store error as `Error::Retry`, with a text that names the secret and says where to
  put the value, so a value added heals the run at its next restart. The store does
  not check the owner or mode of the file. Deferred; trigger: `env::files` gives the
  mode of a file. The person has not yet answered the questions on capitals, on the
  bytes of a file, and on the mode check:
  https://github.com/synnaxlabs/foundation/issues/2583#issuecomment-6150300870.
