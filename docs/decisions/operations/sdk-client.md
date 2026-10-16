- **SDK CLIENT** `sdk::Client` is a blocking client that owns one thread, with a
  Tokio `LocalRuntime` and an env of its own on the host OS. The thread holds a
  `program::Session` on a `transport::Client`, so the `!Send` session stays out of the
  surface, and `sdk` builds on no node core (`hub`, `home`, `mesh`, `raft`). The env
  dials only UDP. Its clock is on `Instant`, the simple version: on Linux and macOS it
  does not count a sleep of the host, so a hello renews late after one. The robust
  version, deferred to #2932 before the SDK ships, is a boot clock that counts the
  sleep. `Credentials` names the subject's key `private_key`. `laptop.architect-2`
  decided on 2026-10-16 at 21:06 UTC
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6224695365). The
  clock is a key choice of kind 8, which `laptop.monitor` accepted with #2932 as its
  condition.
