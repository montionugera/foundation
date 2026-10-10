- **CLOCK SLEEP (2026-10-10)** `clock::Reader::sleep_until(at) -> env::clock::Sleep`
  waits until the node's monotonic clock reaches `at`, never before, and completes at
  once for an `at` that has passed. It reads the monotonic clock that `Reader::now`
  reads, so a deadline from `now().monotonic` and its sleep read one clock. It gives a
  named `Sleep`, not `impl Future`: a task that sleeps again after each wake keeps it
  as a field, and `Sleep::reset` moves its deadline with no new timer. Its panics text
  is the one of `reach`. Lost: `Reader::monotonic(&self) -> &env::clock::Clock`, which
  gives each caller `epoch` and `sleep` too and makes the private field part of the
  contract. Decided by `laptop.architect-2`, #2307, 2026-10-10T23:25:22Z
  (https://github.com/synnaxlabs/foundation/issues/2307#issuecomment-6103294500).
