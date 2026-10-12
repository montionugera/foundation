- **SHARD HOMES (2026-10-07)** Each shard builds its `home::Shard` over its buffer
  once the buffer opens, with the node's `clock::Reader`, and keeps the home until the
  node stops. Its number is its core. It carries no index until the hub picks them
  (#585). The stamp limits (A5) are a patch until #1285 makes them settings: earliest
  2000-01-01T00:00:00Z, which refuses a clock that reads near 1970 but not one that
  resets to 2000-01-01, and refuses backfill from before 2000; ahead 10 s, ten times
  the MVP time error target of 1 s. A field of `node::Config` lost, because a setting
  comes from the spec (NODE SETTINGS), not from the caller of `Node::start`.
  Decided by the architect on #1287:
  https://github.com/synnaxlabs/foundation/pull/1287#issuecomment-6034425115.
  Amended (2026-10-12, #1662, by `laptop.architect-2`, 15:50 UTC):
  https://github.com/synnaxlabs/foundation/issues/1662#issuecomment-6127728083. "A field
  of `node::Config` lost" holds no more. Since #1732 a value that a policy sets reaches
  the node through `node::Config`, as the budget does, so #1285 puts the stamp limits in
  `node::Settings` (NODE PORT). Supersedes "Lost: the limits as `node::Config` fields"
  of https://github.com/synnaxlabs/foundation/pull/1287#issuecomment-6034425115
  (2026-10-07 08:51 UTC).
