- **OPCUA SIM (#2515)** `connector_opcua::sim`, behind the cargo feature `sim`, off by
  default, is the simulated OPC UA server of the tests of other crates. It is the
  open62541 test server of the crate, with no security. `sim::Variables::new(nodes)`
  holds Double variables at the node ids `nodes`, each with the value 0 and the status
  Good. Clones share them. `Variables::change(quality)` adds 1 to each value and gives
  each later read the status `quality`. `Variables::served()` gives each read that a
  server answered, by the text of its node in `new`, in the order of the answers, as
  `sim::Served { time, value, quality }`, with `time` on the injected clock of the
  server. `sim::serve(clock, net, listener, rng, &variables, until)` serves `variables`
  on the bound `listener` with room for 4 sessions until `until` completes, then closes
  the server. It adds each namespace from 2 to the highest of `variables`, gives each
  stream of the server the options of `listener`, and panics on a variable of the number
  0, which open62541 would add at a free number. The caller binds the listener, so it
  knows the address before the serve, as with `connector_influx::sim::serve`. The stop
  is an input, since a drop of the future would leak the server. The test server of the
  crate owns the read of each variable: it answers the reads of the type check when
  open62541 adds the node, and frees each read after the delete. Lost: the values and
  the record of reads in C, with a C array that Rust copies out, which gives two copies
  of the state across the FFI, a C allocation per read, and a second clock in C. Lost: a
  `Server` with `listen`, `local`, and `serve`, since a drop between `listen` and
  `serve` leaks the server. Decided by `laptop.architect-2` (2026-10-13T01:23:17Z:
  https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630), and the
  one `serve`, the `listener`, `&Variables`, the namespaces, and the panic on the number
  0 by `laptop.architect-2` (2026-10-13T02:02:19Z:
  https://github.com/synnaxlabs/foundation/pull/2522#issuecomment-6136492404), and the
  options of `listener` by `laptop.architect-2` (2026-10-13T02:07:45Z:
  https://github.com/synnaxlabs/foundation/pull/2522#issuecomment-6136612131). The one
  `serve` supersedes the `Server` with `listen`, `local`, and `serve`, and its
  `#[must_use]`, of
  https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630. The
  `listener` that the caller binds, which `Manager::accepting` takes, supersedes for
  `sim::serve` the bind in `Manager::listening` of
  https://github.com/synnaxlabs/foundation/pull/2212#issuecomment-6093207582. Both:
  approval owed.
