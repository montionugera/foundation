- **SDK CLIENT** `sdk::Client` is a blocking client that owns one thread, with a
  Tokio `LocalRuntime` and an env of its own on the host OS. The thread holds a
  `program::Session` on a `transport::Client`, so the `!Send` session stays out of the
  surface, and `sdk` builds on no node core (`hub`, `home`, `mesh`, `raft`). The env
  dials only UDP. Its clock is on `Instant`, the simple version: on Linux and macOS it
  does not count a sleep of the host, so a hello renews late after one. The robust
  version, deferred to #2932 before the SDK ships, is a boot clock that counts the
  sleep. The subject's key is `private_key`. `laptop.architect-2` decided on
  2026-10-16 at 21:06 UTC
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6224695365). The
  clock is a key choice of kind 8, sent to `laptop.monitor`; #2932 holds the robust
  version (`laptop.monitor`, 2026-10-16T21:23:11Z,
  https://github.com/synnaxlabs/foundation/issues/2932#issuecomment-6224964525).
  `laptop.architect` approved the boundary (2026-10-16T21:33:33Z,
  https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225122027).
  Until #2923 adds a CI run on Windows, `sdk` builds only on Linux and macOS.
  `Client::connect(&Node, Subject)` takes the node (its UDP addresses, its key, and
  its public key) apart from the subject (its name and private key): one node serves
  many subjects, and a node can listen on more than one address. A read of the clock
  has no fence, unlike `env::clock::Driver::now`: the clock times only the renewal of
  the hello and the timers of the transport, and no interval of these is short enough
  for a moved read to matter. `laptop.architect-2` decided both on 2026-10-16 at
  21:39 UTC
  (https://github.com/synnaxlabs/foundation/pull/2933#issuecomment-6225208118).
