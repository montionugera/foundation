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
    rate    = 10
    read "plc.temp" {
      table   = "holding"
      address = 40
    }
  }
  ```

  1. **Endpoint.** `address` is a host and a port, such as `"plc:502"`, port 502 when
     it has none, and an IPv6 host in brackets (`"[fd00::1]:502"`). The run resolves
     the host through `env::net`. `unit` is optional, 0 to 255, default 1.
  2. **A read.** `table` is `"coil"`, `"discrete"`, `"holding"`, or `"input"`.
     `address` is the address of the protocol, from 0. Lost: the `40001` form, which
     mixes the table into the number and is one off.
  3. **Types.** `coil` and `discrete`: `bool`. `holding` and `input`: `u16` and `i16`
     (1 register), `u32`, `i32`, and `f32` (2), `u64`, `i64`, and `f64` (4). Each
     other type, an array, and a string are `modbus.sample-type` at run, as
     `opcua.sample-type` is. A value whose registers run past address 65535 is
     `modbus.range` at run. Both are at the span of the `read` block. Only the run
     has the sample type of each channel.
  4. **Word order.** `order` on the connector block, a policy over all reads: `"big"`
     (default, the first register holds the high word) or `"little"`. Each register
     is big-endian. Trigger for a byte-swapped order or an order per read: a device
     that needs one.
  5. **Requests.** Each tick reads the reads of one table that touch or overlap as one
     request, at most 125 registers or 2000 bits, one request at a time on one
     connection. Lost: a request across a gap, since an address in the gap that the
     device does not have fails the whole request. `connector_modbus::poll` holds
     items 3, 4, and 5, so the kind never sees an offset into a reply
     (`laptop.architect-2`, 2026-10-15T23:57:47Z:
     https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6204791239).
  6. **Time and frames.** One frame per tick, stamped at the midpoint of the mesh times
     before the first request and after the last reply, with half that span as its
     error (STAMP). One index for the connector. Counts `in`, `missed`, and `lost`.
  7. **Errors.** An exception of `IllegalFunction`, `IllegalAddress`, or
     `IllegalValue` is `Error::Config` with `modbus.exception`, and the text names
     the channel, the table, the address, and the exception: only a spec change fixes
     it. Each other exception, a timeout, and a cut link are `Error::Device`, so the
     supervisor connects again with backoff.
  8. **Samples.** The decode writes each value through `types::sample::Value`.

  Codes of `parse`: `modbus.bad-address`, `modbus.bad-unit`, `modbus.bad-order`,
  `modbus.bad-table`, `modbus.bad-read-address`, `modbus.no-read`, and
  `modbus.repeated-channel`. `check` gives the channel of each `read` as a write, in
  order, no reader, and the counts. `discover` finds nothing (`laptop.architect-2`,
  2026-10-16T00:16:17Z:
  https://github.com/synnaxlabs/foundation/issues/432#issuecomment-6205044071).

  Deferred: one connection that two connectors share through `endpoint::Registry`.
  Trigger: a gateway with two units, or a device that takes one connection (#2712).
