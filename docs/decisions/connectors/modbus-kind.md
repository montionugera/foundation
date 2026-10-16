- **MODBUS KIND (#432)** The `modbus` kind reads the tables of a Modbus TCP device at
  a fixed rate, and writes one frame per tick. The person approved C2, reads first
  ("Approve", relayed by `laptop.monitor`), as the plan of C2 states
  (`laptop.architect-2`, 2026-10-15T16:52:10Z:
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6197484714).
  Commands come later (#2711).

  ```hcl
  connector "plc" {
    kind    = "modbus"
    node    = "edge"
    address = "plc:502"
    rate    = "10Hz"
    read "plc.temp" {
      table   = "holding"
      address = 40
    }
  }
  ```

  1. **Endpoint.** `address` is a host and a port, such as `"plc:502"`, port 502 when it
     has none. The port is 1 to 65535 (`laptop.architect-2`, 2026-10-16T00:25:03Z:
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205157567). An
     IPv6 host is in brackets (`"[fd00::1]:502"`, `"[fe80::1%2]"`), and the kind
     parses the text in them bare. The host is an `env::net::host::Host`, parsed at
     plan, with the grammar of ENV SEAMS (`laptop.architect-2`, 2026-10-16T02:53:11Z:
     https://github.com/synnaxlabs/foundation/issues/1927#issuecomment-6206963417). The
     kind imports `env::net::host` and keeps the split of the host and the port until
     the host and port type of `connector` (#1218) takes it; its own host rules go
     (`box1.builder-2`, 2026-10-16T03:24:43Z:
     https://github.com/synnaxlabs/foundation/issues/1218#issuecomment-6207374015;
     approved by `laptop.architect-2`, 2026-10-16T08:46:11Z:
     https://github.com/synnaxlabs/foundation/pull/2852#issuecomment-6211699251; the
     bare parse in brackets and the import of `env::net::host`,
     `laptop.architect-2`, 2026-10-16T08:55:35Z:
     https://github.com/synnaxlabs/foundation/pull/2852#issuecomment-6211871391).
     Supersedes the host rules of
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205359213
     (`laptop.architect-2`, 2026-10-16T00:41:10Z) and
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205484684
     (2026-10-16T00:50:34Z), the zone cases of
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205579706
     (2026-10-16T00:58:33Z), and, in item 1 of
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205573301
     (2026-10-16T00:58:00Z), that the kind reaches the grammar only through the type
     of #1218 and never imports `env`. The
     run resolves the host through `env::net`. `unit` is optional, 0 to 255, default 1
     (C2 item 1). Lost: the host rule of a library (`rustls_pki_types::ServerName`),
     which takes `"10.0xa"` and `"0xa000001"` as names that a resolver reads as IPv4
     addresses, and refuses a zone; and a zone by interface name (`"%eth0"`), which
     ENV SEAMS refuses until a user needs it.
  2. **A read.** `table` is `coil`, `discrete`, `holding`, or `input`, as a string or
     a reference (HCL REFERENCES; `laptop.architect-2`, 2026-10-16T00:41:10Z, link
     above), as `order` is. `address` is the address of the protocol, from 0. Lost:
     the `40001` form, which mixes the table into the number and is one off.
  3. **Types.** `coil` and `discrete`: `bool`. `holding` and `input`: `u16` and `i16`
     (1 register), `u32`, `i32`, and `f32` (2), `u64`, `i64`, and `f64` (4). Each
     other type, an array, and a string are `modbus.sample-type` at run, as
     `opcua.sample-type` is. A value whose registers run past address 65535 is
     `modbus.range` at run. Each has no span and names the channel, the table, and
     the address, as a config that a node gets from the mesh has no spans. Only the
     run has the sample type of each channel. When `check` gets the sample types
     (#2788), these move to plan. This supersedes note 1 of
     https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6204791239
     (`laptop.architect-2`, 2026-10-16T00:41:10Z:
     https://github.com/synnaxlabs/foundation/pull/2785#issuecomment-6205359213).
  4. **Word order.** `order` on the connector block, a policy over all reads: `big`
     (default, the first register holds the high word) or `little`. Each register
     is big-endian. Trigger for a byte-swapped order or an order per read: a device
     that needs one.
  5. **Requests.** Each tick reads the reads of one table that touch or overlap as one
     request, at most 125 registers or 2000 bits, one request at a time on one
     connection. Lost: a request across a gap, as an address in the gap that the
     device does not have fails the whole request. `connector_modbus::read` holds
     items 3, 4, and 5, so the kind never sees an offset into a reply
     (`laptop.architect-2`, 2026-10-15T23:57:47Z:
     https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6204791239).
  6. **Time and frames.** One frame per tick, stamped at the midpoint of the mesh times
     before the first request and after the last reply, with half that span as its
     error (STAMP). One index for the connector. Counts `in`, `missed`, and `lost`.
  7. **Errors.** An exception of a read of `IllegalFunction`, `IllegalAddress`, or
     `IllegalValue` is `Error::Config` with `modbus.exception`, and the text names
     the channel, the table, the address, and the exception: only a spec change fixes
     it. Each other exception, a timeout, and a cut link are `Error::Device`, so the
     supervisor connects again with backoff. On a write, each exception reply is
     `refused` (#2711,
     https://github.com/synnaxlabs/foundation/issues/2711#issuecomment-6204912305).
  8. **Samples.** The decode writes each value through `types::sample::Value`.

  Codes of `parse`: `modbus.bad-address`, `modbus.bad-unit`, `modbus.bad-order`,
  `modbus.bad-table`, `modbus.bad-read-address`, `modbus.no-read`, and
  `modbus.repeated-channel`. `check` gives the channel of each `read` as a write, in
  order, no reader, and the counts. `discover` gives the default `connector.no-discover`
  (`laptop.architect-2`, 2026-10-16 04:00 UTC:
  https://github.com/synnaxlabs/foundation/issues/2705#issuecomment-6207771367; it
  supersedes
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6205044071, where
  `discover` found nothing).

  Deferred: one connection that two connectors share through `endpoint::Registry`.
  Trigger: a gateway with two units, or a device that takes one connection (#2712).
