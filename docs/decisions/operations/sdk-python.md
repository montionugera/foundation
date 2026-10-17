- **SDK PYTHON** The Python SDK is the crate `sdk-python`, a PyO3 module
  `foundation._native` of the wheel `synnax-foundation` that wraps `sdk::Client`
  (`laptop.architect`, 2026-10-17T18:41:45Z,
  https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6238783320;
  `laptop.architect-2`, 2026-10-17T19:07:21Z,
  https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6239220977).
  `connect(address, *, node, public_key, subject, private_key)` takes the node key as a
  `uuid.UUID`, and a value of another type raises `TypeError`. `sdk::Error::Thread`
  raises `RuntimeError`, `Bind` raises `OSError`, `Session` raises `ConnectionError`,
  and `Panicked` raises `PanicException` (`laptop.architect-2`, 2026-10-17T18:44:19Z,
  https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6238826510).
  `Client.__exit__` lets the error of the block win: when the block raised, an error of
  the close is a note on it, else the error of the close raises (`laptop.architect-2`,
  2026-10-17T18:46:12Z,
  https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6238857409). The
  surface and the errors are key choices of kind 9 that `laptop.monitor` accepted at
  2026-10-17T18:44:46Z
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6240907801). This
  surface is a step: the demo needs `foundation.connect("address")` alone, so the
  defaults of #2177 land before it
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6238857409). The
  feature `sim` adds test-only functions, so a test asserts that a panic on the caller's
  thread gives `PanicException`, not an abort (`laptop.architect-2`,
  2026-10-17T18:39:00Z,
  https://github.com/synnaxlabs/foundation/issues/3059#issuecomment-6238735903). One
  `abi3` wheel for each platform takes Python 3.11 and later, and does not install on
  free-threaded Python. The Linux wheels are `manylinux_2_28`, with no `musllinux`
  wheel. The gate runs against a release node built in the same run. `laptop.monitor`
  accepted both, key choices of kinds (a) and (g), at 2026-10-17T18:40:34Z
  (https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6240907801). The
  Windows wheels wait for #682 and #2955 (option (A),
  https://github.com/synnaxlabs/foundation/issues/2923#issuecomment-6238826510).
