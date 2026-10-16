- **OPCUA SIM (#2515)** `connector_opcua::sim`, behind the cargo feature `sim`, off by
  default, is the simulated OPC UA server of the tests of other crates. It is the
  open62541 test server of the crate, with no security. `sim::Variables::new(nodes)`
  holds variables of the `DataType` `BaseDataType` at the node ids `nodes`, each with
  the Double value 0 and the status Good (`laptop.architect-2`, 2026-10-14T02:30:14Z:
  https://github.com/synnaxlabs/foundation/pull/2635#issuecomment-6159556142). This
  supersedes the Double variables of
  https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630. Clones
  share them. `Variables::change(quality)` adds 1 to each value and gives each later
  read the status `quality`. `Variables::served()` gives each read that a
  server answered, by the text of its node in `new`, in the order of the answers, as
  `sim::Served { time, value, quality }`, with `time` on the injected clock of the
  server. `sim::serve(clock, wall, net, listener, rng, &variables, until)` serves
  `variables` on the bound `listener` with room for 4 sessions until `until` completes,
  then closes the server. `wall` gives the wall time of the server, which a client
  compares with the time of each channel token, so the caller gives the wall time of
  the client's node (`laptop.architect-2`,
  https://github.com/synnaxlabs/foundation/pull/2574#issuecomment-6147681882,
  2026-10-13 14:19 UTC). It adds each namespace from 2 to the highest of `variables`,
  gives each stream of the server the options of `listener`, and panics on a variable
  of the number 0, which open62541 would add at a free number. The caller binds the
  listener, so it knows the address before the serve, as with
  `connector_influx::sim::serve`. The stop is an input, since a drop of the future would
  leak the server. The test server of the crate owns the read of each variable: it
  answers the reads of the type check when open62541 adds the node, and frees each read
  after the delete. Lost: the values and the record of reads in C, with a C array that
  Rust copies out, which gives two copies of the state across the FFI, a C allocation
  per read, and a second clock in C. Decided by `laptop.architect-2`
  (2026-10-13T01:23:17Z:
  https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630), and the
  one `serve`, with the loss of a `Server` with `listen`, `local`, and `serve`, since a
  drop between `listen` and `serve` leaks the server, the `listener`, `&Variables`, the
  namespaces, and the panic on the number 0 by `laptop.architect-2`
  (2026-10-13T02:02:19Z:
  https://github.com/synnaxlabs/foundation/pull/2522#issuecomment-6136492404), and the
  options of `listener` by `laptop.architect-2` (2026-10-13T02:07:45Z:
  https://github.com/synnaxlabs/foundation/pull/2522#issuecomment-6136612131). The one
  `serve` supersedes the `Server` with `listen`, `local`, and `serve`, and its
  `#[must_use]`, of
  https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630. The
  `listener` that the caller binds, which `Manager::accepting` takes, supersedes for
  `sim::serve` the bind in `Manager::listening` of
  https://github.com/synnaxlabs/foundation/pull/2212#issuecomment-6093207582. Both by
  `laptop.architect-2` (2026-10-13T02:19:56Z:
  https://github.com/synnaxlabs/foundation/pull/2522#issuecomment-6136784257).
  `sim::serve` also serves one fixed tree to browse and import, which the module doc of
  `sim` gives: the folder `ns=1;s=Plant` in the Objects folder, with a folder `Types` of
  one variable of each of 15 built-in types, and an Object `Line` with a unit, a
  subtype of Double, of `Enumeration`, and of `Structure`, an array, a variable that a
  client cannot read, and an `Organizes` loop to `Plant`, and a folder `Odd` with a
  folder of a GUID node id, a variable by two references, and a second variable of
  its browse name. Each other node id is `ns=1;s=` and the path with `.`. A page of a Browse holds at most 8 references
  (`maxReferencesPerNode`), so `Plant.Types` takes two pages. A variable of `variables`
  at the node id of a node of the tree panics. No new parameter: each caller gets the
  same tree. Lost: a public builder `sim::Tree` given to `serve`, which repeats the
  test server for callers that need one fixed shape (`laptop.architect-2`,
  2026-10-16T20:14:20Z:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6223855203).
