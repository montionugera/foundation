- **OPCUA SIM (#2515)** `connector_opcua::sim`, behind the cargo feature `sim`, off by
  default, is the simulated OPC UA server of the tests of other crates. It is the
  open62541 test server of the crate, with no security, on a `connection::Manager` from
  `Manager::listening`. `sim::Variables::new(nodes)` holds Double variables at the node
  ids `nodes`, each with the value 0 and the status Good. Clones share them.
  `Variables::change(quality)` adds 1 to each value and gives each later read the
  status `quality`. `Variables::served()` gives each read that a server answered, by the
  text of its node in `new`, in the order of the answers, as `sim::Served { time,
  value, quality }`, with `time` on the injected clock of the server. `Quality` is the
  status code, so the record needs no map. `sim::Server::listen(clock, net, local, rng,
  &variables)` starts a server with room for 4 sessions, and adds each namespace from 2
  to the highest of `variables` with the URI `urn:foundation:sim:{index}`.
  `Server::serve(until)` drives the loop of the manager until `until` completes, then
  closes the server before the manager drops. The stop is an input, since a drop of the
  future would leak the server, and `Server` is `#[must_use]` for the same reason. Each
  variable is a node whose reads a Rust read function answers through
  `shim_server_add_source` in `server.c`. The reads of the type check when open62541
  adds the node are not recorded. Lost: the values and the record of reads in C, with a
  C array that Rust copies out, which gives two copies of the state across the FFI, a C
  allocation per read, and a second clock in C. Decided by `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/issues/2515#issuecomment-6136027630).
