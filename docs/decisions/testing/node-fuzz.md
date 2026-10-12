- **NODE FUZZ (#1994, 2026-10-09)** The fuzz target `node_identity` checks the decode
  of `node.key`, which T1 asks for, since the file is outside input. It calls
  `node::fuzz::identity`, a function in a `#[doc(hidden)]` public module behind the
  feature `sim`, as `bench` is, and `fuzz/Cargo.toml` turns `sim` on. The target takes
  100 bytes as they are, or 96 bytes with their CRC32C appended, since a random input
  almost never has the right CRC32C. The oracle is `identity::check`, which the
  property tests share. It gives `sector::check` its tag and the `encode` of its
  fields: `sector::written` takes the bytes exactly when that `encode` gives them back
  (#2305, asked in
  https://github.com/synnaxlabs/foundation/issues/2305#issuecomment-6103137619,
  approved by `laptop.architect-2`, 2026-10-11 10:16 UTC:
  https://github.com/synnaxlabs/foundation/pull/2384#issuecomment-6107980115). The
  inputs of the form `foundation/key/1` stay, and reach only the length check after the
  form `foundation/key/2` (NODE PORT). By `laptop.architect-2` (#1744, plan revision 3,
  item 6, 2026-10-10 03:30 UTC):
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6093320483. Each
  input of the form `foundation/key/1` that decoded has a copy in the new form that a
  test decodes (`docs/claude/testing.md`, "Oracles", from item 33 of #1839 by
  `laptop.director`, 2026-10-08 19:30 UTC:
  https://github.com/synnaxlabs/foundation/issues/1839#issuecomment-6067499161).
  The target also checks the decode of `admin.key` (NODE MESH), on 54 bytes as they
  are, or on 50 bytes with their CRC32C appended, with `admin::check` as its oracle,
  which the property tests of `admin` share. The inputs `admin-*` are its form (#1744
  PR 1b-3, the trigger that `laptop.architect` moved from PR 1b-1, 2026-10-10 04:15
  UTC: https://github.com/synnaxlabs/foundation/pull/2229#issuecomment-6093666647). A
  test founds a region with the key of `admin-valid`.
  The target also checks the decode of the file `address` (NODE START), on 99 bytes as
  they are, or on 95 bytes with their CRC32C appended, with `address::check` as its
  oracle, which the property tests of `address` share. The inputs `address-*` are its
  form. A node that runs holds the bytes of `address-valid`, a clean stop leaves those
  of `address-zeros`, and a test decodes `address-valid-v6`. By `laptop.architect-2`
  (#1744, plan revision 4, PR 2a, 2026-10-11 07:10 UTC:
  https://github.com/synnaxlabs/foundation/issues/1744#issuecomment-6106546913).
  `address::check` gives `sector::check` its tag and the `encode` of its fields, or
  `None` for fields that no address has (`laptop.architect-2`, 2026-10-11 12:30 UTC:
  https://github.com/synnaxlabs/foundation/pull/2391#issuecomment-6109038962).
  Lost: property tests only, with a sentence in `docs/security.md` that the decode
  needs no target.
  It breaks T1. Decided by `laptop.architect-2`
  (https://github.com/synnaxlabs/foundation/issues/1994#issuecomment-6071757310,
  2026-10-09 00:28 UTC); the surface, in
  https://github.com/synnaxlabs/foundation/pull/2046#issuecomment-6071891219
  (2026-10-09 00:41 UTC); `check` as a property of `encode`, by `laptop.director`
  (https://github.com/synnaxlabs/foundation/pull/2046#issuecomment-6072502872,
  2026-10-09 01:39 UTC).
